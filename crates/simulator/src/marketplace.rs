//! Subscription-based marketplace simulation.
//!
//! Each simulated agent registers a callback URL with the coordinator and
//! responds to `TaskNotification` POSTs with accept/decline logic based on:
//!
//! * **Capacity**: the agent declines if it is already handling
//!   `max_concurrent_tasks` tasks.
//! * **Random probability**: the agent declines with probability
//!   `decline_probability` (configurable per-agent).
//!
//! Accepted tasks are dispatched to the agent's LLM client for actual
//! work/rating (same as the polling-based simulation, just push-driven).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use eyre::{Result, WrapErr as _};
use protocol::{AgentRole, TaskNotification};
use rand::Rng as _;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use agent::{Backend, LlmClient, Message};

// ── Simulated agent state ─────────────────────────────────────────────────────

pub(crate) struct SimAgent {
    pub agent_id: String,
    pub role: AgentRole,
    pub max_concurrent: usize,
    pub decline_probability: f64,
    pub coordinator_url: String,
    /// Tasks currently being handled.
    active: Mutex<Vec<Uuid>>,
    llm: LlmClient,
    backend: Backend,
}

impl SimAgent {
    pub(crate) fn new(
        agent_id: impl Into<String>,
        role: AgentRole,
        max_concurrent: usize,
        decline_probability: f64,
        coordinator_url: impl Into<String>,
        llm: LlmClient,
    ) -> Self {
        let coordinator_url = coordinator_url.into();
        let backend = Backend::http(&coordinator_url);
        Self {
            agent_id: agent_id.into(),
            role,
            max_concurrent,
            decline_probability,
            coordinator_url,
            active: Mutex::new(Vec::new()),
            llm,
            backend,
        }
    }

    async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    async fn add_task(&self, id: Uuid) {
        self.active.lock().await.push(id);
    }

    async fn remove_task(&self, id: Uuid) {
        self.active.lock().await.retain(|t| *t != id);
    }
}

// ── Callback server ───────────────────────────────────────────────────────────

async fn handle_notification(
    State(agent): State<Arc<SimAgent>>,
    Json(notif): Json<TaskNotification>,
) -> Result<StatusCode, (StatusCode, String)> {
    let task_id = notif.task_id;
    info!(
        "sim-agent {} received task notification {}",
        agent.agent_id, task_id
    );

    // --- Capacity check ---
    let active = agent.active_count().await;
    if active >= agent.max_concurrent {
        let reason = format!(
            "at capacity ({active}/{} tasks)",
            agent.max_concurrent
        );
        info!("sim-agent {} declining {task_id}: {reason}", agent.agent_id);
        fire_decline(&agent, task_id, &notif.decline_url, &reason).await;
        return Ok(StatusCode::OK);
    }

    // --- Random decline ---
    let roll: f64 = rand::thread_rng().gen();
    if roll < agent.decline_probability {
        let reason = format!(
            "random decline (p={:.2})",
            agent.decline_probability
        );
        info!("sim-agent {} declining {task_id}: {reason}", agent.agent_id);
        fire_decline(&agent, task_id, &notif.decline_url, &reason).await;
        return Ok(StatusCode::OK);
    }

    // --- Accept ---
    let client = client::CoordinatorClient::new(&agent.coordinator_url);
    match client.accept_task(task_id, &agent.agent_id, agent.role).await {
        Ok(resp) => {
            let granted = resp.role_granted.clone();
            info!(
                "sim-agent {} accepted {task_id} as {granted}",
                agent.agent_id
            );
            if granted == "declined" {
                return Ok(StatusCode::OK);
            }

            agent.add_task(task_id).await;

            let agent_clone = Arc::clone(&agent);
            let prompt = notif.prompt.clone();
            let granted_role_is_worker = granted == "worker";
            tokio::spawn(async move {
                do_work(&agent_clone, task_id, &prompt, granted_role_is_worker).await;
                agent_clone.remove_task(task_id).await;
                info!("sim-agent {} done with {task_id}", agent_clone.agent_id);
            });
        }
        Err(e) => {
            error!(
                "sim-agent {} failed to accept {task_id}: {e}",
                agent.agent_id
            );
        }
    }

    Ok(StatusCode::OK)
}

async fn fire_decline(agent: &SimAgent, task_id: Uuid, decline_url: &str, reason: &str) {
    let body = protocol::DeclineTaskRequest {
        agent_id: agent.agent_id.clone(),
        reason: reason.to_string(),
    };
    let http = reqwest::Client::new();
    match http.post(decline_url).json(&body).send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => warn!(
            "sim-agent {} decline {task_id} returned {}",
            agent.agent_id,
            r.status()
        ),
        Err(e) => error!(
            "sim-agent {} decline {task_id} failed: {e}",
            agent.agent_id
        ),
    }
}

async fn do_work(agent: &SimAgent, task_id: Uuid, prompt: &str, is_worker: bool) {
    if is_worker {
        // Generate output via LLM
        let messages = vec![Message {
            role: "user".into(),
            content: format!("{prompt}\n\nProvide a complete, working Rust implementation."),
        }];
        let output = match agent.llm.chat(&messages).await {
            Ok(o) => o,
            Err(e) => {
                error!("sim-agent {} LLM error for {task_id}: {e}", agent.agent_id);
                return;
            }
        };
        match agent
            .backend
            .submit_result(task_id, &agent.agent_id, &output)
            .await
        {
            Ok(_) => info!("sim-agent {} submitted result for {task_id}", agent.agent_id),
            Err(e) => error!(
                "sim-agent {} submit_result failed for {task_id}: {e}",
                agent.agent_id
            ),
        }
    } else {
        // Rater: wait for AwaitingRatings then submit rating
        rate_task(agent, task_id, prompt).await;
    }
}

async fn rate_task(agent: &SimAgent, task_id: Uuid, prompt: &str) {
    use tokio::time::{sleep, Duration};

    for attempt in 0..30u32 {
        #[allow(clippy::arithmetic_side_effects)]
        sleep(Duration::from_secs(u64::from(attempt) + 1)).await;
        match agent.backend.get_task(task_id).await {
            Ok(status) => {
                if let protocol::TaskPhase::AwaitingRatings {
                    worker_output,
                    ratings,
                    ..
                } = &status.phase
                {
                    if ratings.iter().any(|r| r.agent_id == agent.agent_id) {
                        return; // already rated
                    }

                    let rating_prompt = format!(
                        "Rate the following output as GOOD or BAD and predict what fraction of \
                         other raters will say GOOD (0.0-1.0).\n\nTask: {prompt}\n\nOutput: {}\n\n\
                         Respond as JSON only: {{\"signal\": true, \"prediction\": 0.75}}",
                        worker_output
                    );
                    let messages = vec![Message {
                        role: "user".into(),
                        content: rating_prompt,
                    }];
                    let (signal, prediction) = match agent.llm.chat(&messages).await {
                        Ok(raw) => parse_rating(&raw).unwrap_or((true, 0.5)),
                        Err(e) => {
                            error!(
                                "sim-agent {} LLM error rating {task_id}: {e}",
                                agent.agent_id
                            );
                            (true, 0.5)
                        }
                    };

                    match agent
                        .backend
                        .submit_rating(task_id, &agent.agent_id, signal, prediction)
                        .await
                    {
                        Ok(_) => {
                            info!(
                                "sim-agent {} rated {task_id} signal={signal} pred={prediction:.2}",
                                agent.agent_id
                            );
                        }
                        Err(e) => {
                            error!(
                                "sim-agent {} submit_rating failed for {task_id}: {e}",
                                agent.agent_id
                            );
                        }
                    }
                    return;
                }
                if matches!(status.phase, protocol::TaskPhase::Scored { .. }) {
                    info!(
                        "sim-agent {} task {task_id} already scored",
                        agent.agent_id
                    );
                    return;
                }
            }
            Err(e) => {
                error!("sim-agent {} get_task {task_id} failed: {e}", agent.agent_id);
            }
        }
    }
    warn!(
        "sim-agent {} timed out waiting for AwaitingRatings on {task_id}",
        agent.agent_id
    );
}

fn parse_rating(raw: &str) -> Option<(bool, f64)> {
    #[derive(serde::Deserialize)]
    struct R {
        signal: bool,
        prediction: f64,
    }
    let start = raw.find('{')?;
    #[allow(clippy::arithmetic_side_effects)]
    let end = raw.rfind('}')? + 1;
    let parsed: R = serde_json::from_str(&raw[start..end]).ok()?;
    Some((parsed.signal, parsed.prediction.clamp(0.0, 1.0)))
}

// ── Agent lifecycle ───────────────────────────────────────────────────────────

/// Start an HTTP callback server for `agent` on an OS-assigned port.
/// Returns the bound port.
pub(crate) async fn start_agent_server(
    agent: Arc<SimAgent>,
    token: CancellationToken,
) -> Result<u16> {
    let app = Router::new()
        .route("/notify", post(handle_notification))
        .with_state(Arc::clone(&agent));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .wrap_err("bind agent callback server")?;
    let port = listener
        .local_addr()
        .wrap_err("get local addr")?
        .port();

    tokio::spawn(async move {
        tokio::select! {
            r = axum::serve(listener, app) => {
                if let Err(e) = r { error!("sim-agent callback error: {e}"); }
            }
            _ = token.cancelled() => {}
        }
    });

    Ok(port)
}

/// Register `agent` with the coordinator at `callback_url`.
pub(crate) async fn subscribe_agent(agent: &SimAgent, callback_url: &str) -> Result<()> {
    let c = client::CoordinatorClient::new(&agent.coordinator_url);
    c.subscribe(&agent.agent_id, callback_url, vec![agent.role])
        .await
        .wrap_err("subscribe_agent")?;
    info!(
        "sim-agent {} subscribed (callback={callback_url})",
        agent.agent_id
    );
    Ok(())
}

/// Unregister `agent` from the coordinator.
pub(crate) async fn unsubscribe_agent(agent: &SimAgent) {
    let c = client::CoordinatorClient::new(&agent.coordinator_url);
    match c.unsubscribe(&agent.agent_id).await {
        Ok(_) => info!("sim-agent {} unsubscribed", agent.agent_id),
        Err(e) => warn!("sim-agent {} unsubscribe failed: {e}", agent.agent_id),
    }
}
