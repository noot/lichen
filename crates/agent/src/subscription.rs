//! Agent subscription — register with the coordinator, receive task notifications,
//! and decide whether to accept or decline each task.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use eyre::{Result, WrapErr as _};
use protocol::{AgentRole, TaskNotification};
use rand::Rng as _;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::backend::Backend;
use crate::llm::LlmClient;
use crate::polling;

// ── Config ────────────────────────────────────────────────────────────────────

/// Configuration for the subscription-based task dispatch mechanism.
#[derive(Clone)]
pub struct SubscriptionConfig {
    /// Stable identifier for this agent.
    pub agent_id: String,
    /// URL the coordinator should POST `TaskNotification` to.
    /// Must be reachable from the coordinator.
    pub callback_url: String,
    /// Roles this agent can fill.
    pub roles: Vec<AgentRole>,
    /// Maximum number of tasks the agent will accept concurrently.
    /// When the active task count reaches this limit the agent declines
    /// new tasks with "at capacity".
    pub max_concurrent_tasks: usize,
    /// Probability (0.0–1.0) of randomly declining a task even when under capacity.
    /// Useful for the simulator to model realistic behaviour.
    pub decline_probability: f64,
    /// Coordinator base URL (used to call accept/decline endpoints).
    pub coordinator_url: String,
}

// ── Shared state between callback server and task workers ────────────────────

pub struct SubscriptionState {
    pub config: SubscriptionConfig,
    /// IDs of tasks currently being worked on.
    pub active_tasks: Mutex<Vec<Uuid>>,
    pub(crate) llm: LlmClient,
    pub(crate) backend: Backend,
}

impl SubscriptionState {
    pub fn new(config: SubscriptionConfig, llm: LlmClient, backend: Backend) -> Self {
        Self {
            config,
            active_tasks: Mutex::new(Vec::new()),
            llm,
            backend,
        }
    }

    /// Current number of active tasks.
    pub async fn active_count(&self) -> usize {
        self.active_tasks.lock().await.len()
    }

    /// Register a task as active. Returns false if already tracked.
    pub async fn add_task(&self, task_id: Uuid) -> bool {
        let mut tasks = self.active_tasks.lock().await;
        if tasks.contains(&task_id) {
            return false;
        }
        tasks.push(task_id);
        true
    }

    /// Remove a task from the active set.
    pub async fn remove_task(&self, task_id: Uuid) {
        self.active_tasks.lock().await.retain(|id| *id != task_id);
    }
}

// ── Callback HTTP handler ─────────────────────────────────────────────────────

/// POST /notify — receives `TaskNotification` POSTed by the coordinator.
async fn handle_notification(
    State(state): State<Arc<SubscriptionState>>,
    Json(notification): Json<TaskNotification>,
) -> Result<StatusCode, (StatusCode, String)> {
    let task_id = notification.task_id;
    info!(
        "received task notification for {} from coordinator",
        task_id
    );

    // --- Decline logic: capacity check ---
    let active = state.active_count().await;
    if active >= state.config.max_concurrent_tasks {
        let reason = format!(
            "at capacity ({active}/{} active tasks)",
            state.config.max_concurrent_tasks
        );
        info!("declining task {task_id}: {reason}");
        decline_task(&state.config, task_id, &reason, &notification.decline_url).await;
        return Ok(StatusCode::OK);
    }

    // --- Decline logic: role compatibility ---
    let primary_role = state.config.roles.first().copied();
    if let Some(role) = primary_role {
        // Both worker and rater roles are generally compatible with any notification;
        // role assignment happens at accept time.  If the agent only has one role
        // registered it still can accept (coordinator maps to appropriate slot).
        let _ = role; // kept here for extensibility
    }

    // --- Decline logic: random probability ---
    let roll: f64 = rand::thread_rng().gen();
    if roll < state.config.decline_probability {
        let reason = format!(
            "random decline (probability={:.2})",
            state.config.decline_probability
        );
        info!("declining task {task_id}: {reason}");
        decline_task(&state.config, task_id, &reason, &notification.decline_url).await;
        return Ok(StatusCode::OK);
    }

    // --- Accept ---
    let role = primary_role.unwrap_or(AgentRole::Worker);
    let client = client::CoordinatorClient::new(&state.config.coordinator_url);
    match client
        .accept_task(task_id, &state.config.agent_id, role)
        .await
    {
        Ok(resp) => {
            info!(
                "accepted task {task_id} as {}: {}",
                resp.role_granted, resp.message
            );
            // Parse the granted role so we actually do the right work
            let granted_role = match resp.role_granted.as_str() {
                "worker" => crate::cli::AgentRole::Worker,
                "rater" => crate::cli::AgentRole::Rater,
                "declined" => {
                    // coordinator decided to decline (e.g. worker slot already taken and
                    // we didn't want to be a rater)
                    return Ok(StatusCode::OK);
                }
                other => {
                    warn!("unexpected role_granted={other}, defaulting to worker");
                    crate::cli::AgentRole::Worker
                }
            };

            // Mark as active and spawn work in background
            state.add_task(task_id).await;
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                execute_accepted_task(state_clone, task_id, &notification.prompt, granted_role)
                    .await;
            });
        }
        Err(e) => {
            error!("failed to accept task {task_id}: {e}");
        }
    }

    Ok(StatusCode::OK)
}

async fn decline_task(
    config: &SubscriptionConfig,
    task_id: Uuid,
    reason: &str,
    decline_url: &str,
) {
    // Use the decline_url from the notification directly (it's the full URL),
    // but we also have the client method as fallback.
    let client = reqwest::Client::new();
    let body = protocol::DeclineTaskRequest {
        agent_id: config.agent_id.clone(),
        reason: reason.to_string(),
    };
    match client.post(decline_url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("declined task {task_id} successfully");
        }
        Ok(resp) => {
            warn!("decline task {task_id} returned {}", resp.status());
        }
        Err(e) => {
            error!("decline task {task_id} failed: {e}");
        }
    }
}

/// Executes an accepted task (work or rate) and cleans up when done.
async fn execute_accepted_task(
    state: Arc<SubscriptionState>,
    task_id: Uuid,
    prompt: &str,
    role: crate::cli::AgentRole,
) {
    let agent_id = &state.config.agent_id;
    info!("executing task {task_id} as {:?}", role);

    match role {
        crate::cli::AgentRole::Worker => {
            // Use the existing polling helper which handles LLM call + result submit
            polling::handle_work_notification(agent_id, &state.llm, &state.backend, task_id, prompt).await;
        }
        crate::cli::AgentRole::Rater => {
            // For subscription-based rating we need the worker output.
            // Poll the task status to get it.
            match state.backend.get_task(task_id).await {
                Ok(status) => {
                    if let protocol::TaskPhase::AwaitingRatings {
                        worker_output,
                        ratings,
                        ..
                    } = &status.phase
                    {
                        let already_rated = ratings.iter().any(|r| r.agent_id == *agent_id);
                        if !already_rated {
                            polling::handle_rate_notification(
                                agent_id,
                                &state.llm,
                                &state.backend,
                                task_id,
                                &status.task.prompt,
                                worker_output,
                            )
                            .await;
                        }
                    } else {
                        // Task not yet in AwaitingRatings; wait a bit and retry
                        wait_for_rating_phase_and_rate(state.clone(), task_id, agent_id).await;
                    }
                }
                Err(e) => {
                    error!("failed to get task {task_id} for rating: {e}");
                }
            }
        }
    }

    state.remove_task(task_id).await;
    info!("finished task {task_id}");
}

/// Polls until the task reaches `AwaitingRatings` then submits a rating.
async fn wait_for_rating_phase_and_rate(
    state: Arc<SubscriptionState>,
    task_id: Uuid,
    agent_id: &str,
) {
    use tokio::time::{sleep, Duration};

    for attempt in 0..30u32 {
        sleep(Duration::from_secs(u64::from(attempt).saturating_add(1))).await;
        match state.backend.get_task(task_id).await {
            Ok(status) => {
                if let protocol::TaskPhase::AwaitingRatings {
                    worker_output,
                    ratings,
                    ..
                } = &status.phase
                {
                    let already_rated = ratings.iter().any(|r| r.agent_id == agent_id);
                    if !already_rated {
                        polling::handle_rate_notification(
                            agent_id,
                            &state.llm,
                            &state.backend,
                            task_id,
                            &status.task.prompt,
                            worker_output,
                        )
                        .await;
                    }
                    return;
                }
                if matches!(status.phase, protocol::TaskPhase::Scored { .. }) {
                    info!("task {task_id} already scored when we tried to rate it");
                    return;
                }
            }
            Err(e) => {
                error!("get_task error while waiting for rating phase on {task_id}: {e}");
            }
        }
    }
    warn!("timed out waiting for task {task_id} to reach AwaitingRatings");
}

// ── Subscription lifecycle ────────────────────────────────────────────────────

/// Start the callback HTTP server on `port`, subscribe with the coordinator,
/// then run until `token` is cancelled, at which point unsubscribe.
///
/// Returns the local port the callback server bound to.
pub async fn run_subscription_server(
    port: u16,
    state: Arc<SubscriptionState>,
    token: CancellationToken,
) -> Result<u16> {
    let app = Router::new()
        .route("/notify", post(handle_notification))
        .with_state(Arc::clone(&state));

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .wrap_err("failed to bind callback listener")?;
    let bound_port = listener
        .local_addr()
        .wrap_err("failed to get local addr")?
        .port();

    info!(
        "agent {} callback server listening on :{bound_port}",
        state.config.agent_id
    );

    let token_clone = token.clone();
    tokio::spawn(async move {
        tokio::select! {
            result = axum::serve(listener, app) => {
                if let Err(e) = result {
                    error!("callback server error: {e}");
                }
            }
            _ = token_clone.cancelled() => {
                info!("callback server shutting down");
            }
        }
    });

    Ok(bound_port)
}

/// Register with the coordinator.
pub async fn subscribe(config: &SubscriptionConfig) -> Result<()> {
    let client = client::CoordinatorClient::new(&config.coordinator_url);
    client
        .subscribe(&config.agent_id, &config.callback_url, config.roles.clone())
        .await
        .wrap_err("subscribe failed")?;
    info!(
        "agent {} subscribed at {}",
        config.agent_id, config.callback_url
    );
    Ok(())
}

/// Unregister from the coordinator.
pub async fn unsubscribe(config: &SubscriptionConfig) {
    let client = client::CoordinatorClient::new(&config.coordinator_url);
    match client.unsubscribe(&config.agent_id).await {
        Ok(_) => info!("agent {} unsubscribed", config.agent_id),
        Err(e) => warn!("unsubscribe failed for {}: {e}", config.agent_id),
    }
}
