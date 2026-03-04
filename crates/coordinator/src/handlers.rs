use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, Sse},
    Json,
};
use futures_util::stream::Stream;
use protocol::{
    AcceptTaskRequest, AgentRole, CancelTaskRequest, CreateTaskRequest, DeclineTaskRequest,
    FinalizeTaskRequest, SubmitRatingRequest, SubmitResultRequest, SubscribeRequest,
    SubscribeResponse, TaskAcceptResponse, TaskStatus,
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tracing::info;
use uuid::Uuid;

use crate::coordinator::{AgentSubscription, AppState, TaskDispatch, TaskEvent};
use crate::notifier;

// ── Task CRUD ─────────────────────────────────────────────────────────────────

/// POST /tasks — create a new task
pub(crate) async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskStatus>), (StatusCode, String)> {
    let status = state
        .backend
        .create_task(
            req.prompt.clone(),
            req.output,
            req.num_raters,
            req.max_raters,
            req.min_raters,
            req.timeout_seconds,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let task_id = status.task.id;

    // Initialise dispatch state for this task
    state
        .subscriptions
        .dispatches
        .write()
        .await
        .insert(task_id, TaskDispatch::default());

    // Emit SSE event
    let _ = state.event_tx.send(TaskEvent::TaskCreated {
        task_id,
        prompt: req.prompt.clone(),
    });

    // Notify subscribed agents
    let subs = state.subscriptions.list().await;
    if !subs.is_empty() {
        let notification = protocol::TaskNotification {
            task_id,
            prompt: req.prompt,
            onchain_task_id: None,
            max_raters: req.max_raters.unwrap_or(10),
            min_raters: req.min_raters.unwrap_or(1),
            deadline: 0,
            accept_url: format!("{}/tasks/{task_id}/accept", state.base_url),
            decline_url: format!("{}/tasks/{task_id}/decline", state.base_url),
        };
        notifier::broadcast_task_notification(&subs, &notification);
    }

    info!("created task {task_id}");

    Ok((StatusCode::CREATED, Json(status)))
}

/// GET /tasks — list all tasks
pub(crate) async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TaskStatus>>, (StatusCode, String)> {
    let tasks = state
        .backend
        .list_tasks()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(tasks))
}

/// GET /tasks/:task_id — get task status
pub(crate) async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<TaskStatus>, (StatusCode, String)> {
    state
        .backend
        .get_task(task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "task not found".to_string()))
}

/// POST /tasks/:task_id/result — worker pushes output
pub(crate) async fn submit_result(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<SubmitResultRequest>,
) -> Result<Json<TaskStatus>, (StatusCode, String)> {
    let status = state
        .backend
        .submit_result(task_id, req.agent_id.clone(), req.output)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("already has a result") {
                (StatusCode::CONFLICT, msg)
            } else if msg.contains("not found") {
                (StatusCode::NOT_FOUND, msg)
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        })?;

    info!("task {task_id}: result submitted by {}", req.agent_id);
    Ok(Json(status))
}

/// POST /tasks/:task_id/rating — rater pushes rating
pub(crate) async fn submit_rating(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<SubmitRatingRequest>,
) -> Result<Json<TaskStatus>, (StatusCode, String)> {
    let status = state
        .backend
        .submit_rating(
            task_id,
            req.agent_id.clone(),
            req.signal,
            req.prediction,
            state.alpha,
            state.beta,
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("already rated")
                || msg.contains("already scored")
                || msg.contains("no result yet")
            {
                (StatusCode::CONFLICT, msg)
            } else if msg.contains("not found") {
                (StatusCode::NOT_FOUND, msg)
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        })?;

    // Emit event
    let _ = state.event_tx.send(TaskEvent::TaskRated {
        task_id,
        agent_id: req.agent_id.clone(),
    });

    // Check if task was scored
    if let protocol::TaskPhase::Scored { accepted, .. } = &status.phase {
        let _ = state.event_tx.send(TaskEvent::TaskScored {
            task_id,
            accepted: *accepted,
        });
    }

    info!("task {task_id}: rating submitted by {}", req.agent_id);
    Ok(Json(status))
}

// ── Open-marketplace task lifecycle ──────────────────────────────────────────

/// POST /tasks/:task_id/accept — agent accepts a task offer.
///
/// First agent to accept with `role = worker` claims the worker slot.
/// Subsequent agents (or those accepting as `role = rater`) are queued as raters.
pub(crate) async fn accept_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<AcceptTaskRequest>,
) -> Result<Json<TaskAcceptResponse>, (StatusCode, String)> {
    // Ensure task exists
    let task_exists = state
        .backend
        .get_task(task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some();

    if !task_exists {
        return Err((StatusCode::NOT_FOUND, "task not found".to_string()));
    }

    let mut dispatches = state.subscriptions.dispatches.write().await;
    let dispatch = dispatches.entry(task_id).or_default();

    let (role_granted, message) = match req.role {
        AgentRole::Worker => {
            if dispatch.worker.is_none() {
                dispatch.worker = Some(req.agent_id.clone());
                (
                    "worker".to_string(),
                    format!("agent {} claimed worker slot for task {task_id}", req.agent_id),
                )
            } else {
                // Worker slot taken — queue as rater instead
                if !dispatch.raters.contains(&req.agent_id) {
                    dispatch.raters.push(req.agent_id.clone());
                }
                (
                    "rater".to_string(),
                    format!(
                        "worker slot already taken; agent {} queued as rater for task {task_id}",
                        req.agent_id
                    ),
                )
            }
        }
        AgentRole::Rater => {
            if !dispatch.raters.contains(&req.agent_id) {
                dispatch.raters.push(req.agent_id.clone());
            }
            (
                "rater".to_string(),
                format!(
                    "agent {} queued as rater for task {task_id}",
                    req.agent_id
                ),
            )
        }
    };

    drop(dispatches);

    let _ = state.event_tx.send(TaskEvent::TaskAccepted {
        task_id,
        agent_id: req.agent_id.clone(),
        role: req.role,
    });

    info!("task {task_id}: accepted by {} as {role_granted}", req.agent_id);

    Ok(Json(TaskAcceptResponse {
        task_id,
        role_granted,
        message,
    }))
}

/// POST /tasks/:task_id/decline — agent declines a task offer.
pub(crate) async fn decline_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<DeclineTaskRequest>,
) -> Result<Json<TaskAcceptResponse>, (StatusCode, String)> {
    // Ensure task exists
    let task_exists = state
        .backend
        .get_task(task_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .is_some();

    if !task_exists {
        return Err((StatusCode::NOT_FOUND, "task not found".to_string()));
    }

    let mut dispatches = state.subscriptions.dispatches.write().await;
    let dispatch = dispatches.entry(task_id).or_default();

    if !dispatch.declined.contains(&req.agent_id) {
        dispatch.declined.push(req.agent_id.clone());
    }

    drop(dispatches);

    let reason_note = if req.reason.is_empty() {
        String::new()
    } else {
        format!(" (reason: {})", req.reason)
    };

    info!(
        "task {task_id}: declined by {}{reason_note}",
        req.agent_id
    );

    let _ = state.event_tx.send(TaskEvent::TaskDeclined {
        task_id,
        agent_id: req.agent_id.clone(),
    });

    Ok(Json(TaskAcceptResponse {
        task_id,
        role_granted: "declined".to_string(),
        message: format!("agent {} declined task {task_id}", req.agent_id),
    }))
}

/// POST /tasks/:task_id/finalize — trigger on-chain finalization.
///
/// Only meaningful in `--onchain` mode; returns 400 in off-chain mode.
pub(crate) async fn finalize_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<FinalizeTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !state.onchain {
        return Err((
            StatusCode::BAD_REQUEST,
            "finalize_task is only available in on-chain mode".to_string(),
        ));
    }

    // Delegate to backend (OnchainBackend knows the on-chain ID)
    state
        .backend
        .finalize_task(task_id)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, msg)
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        })?;

    let _ = state
        .event_tx
        .send(TaskEvent::TaskFinalized { task_id });

    info!(
        "task {task_id}: finalized on-chain by {}",
        req.agent_id
    );

    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "status": "finalized"
    })))
}

/// POST /tasks/:task_id/cancel — trigger on-chain cancellation.
///
/// Only meaningful in `--onchain` mode; returns 400 in off-chain mode.
pub(crate) async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<CancelTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !state.onchain {
        return Err((
            StatusCode::BAD_REQUEST,
            "cancel_task is only available in on-chain mode".to_string(),
        ));
    }

    state
        .backend
        .cancel_task(task_id)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, msg)
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, msg)
            }
        })?;

    let _ = state
        .event_tx
        .send(TaskEvent::TaskCancelled { task_id });

    info!(
        "task {task_id}: cancelled on-chain by {}",
        req.agent_id
    );

    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "status": "cancelled"
    })))
}

// ── Agent subscriptions ───────────────────────────────────────────────────────

/// POST /subscribe — register an agent to receive task notifications.
pub(crate) async fn subscribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubscribeRequest>,
) -> Result<(StatusCode, Json<SubscribeResponse>), (StatusCode, String)> {
    if req.agent_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "agent_id must not be empty".to_string()));
    }
    if req.callback_url.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "callback_url must not be empty".to_string()));
    }

    let roles = if req.roles.is_empty() {
        vec![AgentRole::Worker, AgentRole::Rater]
    } else {
        req.roles
    };

    let agent_id = req.agent_id.clone();
    state
        .subscriptions
        .subscribe(AgentSubscription {
            agent_id: req.agent_id,
            callback_url: req.callback_url,
            roles,
        })
        .await;

    Ok((
        StatusCode::CREATED,
        Json(SubscribeResponse {
            agent_id: agent_id.clone(),
            message: format!("agent {agent_id} subscribed"),
        }),
    ))
}

/// DELETE /subscribe/:agent_id — unsubscribe an agent.
pub(crate) async fn unsubscribe(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if state.subscriptions.unsubscribe(&agent_id).await {
        Ok(Json(
            serde_json::json!({ "agent_id": agent_id, "status": "unsubscribed" }),
        ))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            format!("agent {agent_id} not found"),
        ))
    }
}

/// GET /subscriptions — list all current subscriptions.
pub(crate) async fn list_subscriptions(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let subs = state.subscriptions.list().await;
    let list: Vec<_> = subs
        .iter()
        .map(|s| {
            serde_json::json!({
                "agent_id": s.agent_id,
                "callback_url": s.callback_url,
                "roles": s.roles,
            })
        })
        .collect();
    Json(serde_json::json!({ "subscriptions": list, "count": list.len() }))
}

// ── SSE event stream ─────────────────────────────────────────────────────────

/// GET /events/stream — SSE stream of task events
pub(crate) async fn events_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let event_stream = stream.filter_map(|result| {
        result.ok().map(|event| {
            let (event_type, data) = match &event {
                TaskEvent::TaskCreated { task_id, prompt } => (
                    "task_created",
                    serde_json::json!({
                        "type": "task_created",
                        "task_id": task_id,
                        "prompt": prompt
                    }),
                ),
                TaskEvent::TaskAccepted {
                    task_id,
                    agent_id,
                    role,
                } => (
                    "task_accepted",
                    serde_json::json!({
                        "type": "task_accepted",
                        "task_id": task_id,
                        "agent_id": agent_id,
                        "role": role,
                    }),
                ),
                TaskEvent::TaskDeclined { task_id, agent_id } => (
                    "task_declined",
                    serde_json::json!({
                        "type": "task_declined",
                        "task_id": task_id,
                        "agent_id": agent_id,
                    }),
                ),
                TaskEvent::TaskRated { task_id, agent_id } => (
                    "task_rated",
                    serde_json::json!({
                        "type": "task_rated",
                        "task_id": task_id,
                        "agent_id": agent_id
                    }),
                ),
                TaskEvent::TaskScored { task_id, accepted } => (
                    "task_scored",
                    serde_json::json!({
                        "type": "task_scored",
                        "task_id": task_id,
                        "accepted": accepted
                    }),
                ),
                TaskEvent::TaskFinalized { task_id } => (
                    "task_finalized",
                    serde_json::json!({
                        "type": "task_finalized",
                        "task_id": task_id,
                    }),
                ),
                TaskEvent::TaskCancelled { task_id } => (
                    "task_cancelled",
                    serde_json::json!({
                        "type": "task_cancelled",
                        "task_id": task_id,
                    }),
                ),
            };

            Ok::<_, std::convert::Infallible>(
                Event::default().data(data.to_string()).event(event_type),
            )
        })
    });

    Sse::new(event_stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(30))
            .text("heartbeat"),
    )
}
