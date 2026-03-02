use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, Sse},
    Json,
};
use futures_util::stream::Stream;
use protocol::{CreateTaskRequest, SubmitRatingRequest, SubmitResultRequest, TaskStatus};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tracing::info;
use uuid::Uuid;

use crate::coordinator::{AppState, TaskEvent};

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

    // Emit event
    let _ = state.event_tx.send(TaskEvent::TaskCreated {
        task_id: status.task.id,
        prompt: req.prompt,
    });

    info!("created task {}", status.task.id);

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

/// GET /events/stream — SSE stream of task events
pub(crate) async fn events_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.event_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let event_stream = stream.filter_map(|result| {
        result.ok().map(|event| {
            let data = match &event {
                TaskEvent::TaskCreated { task_id, prompt } => {
                    serde_json::json!({
                        "type": "task_created",
                        "task_id": task_id,
                        "prompt": prompt
                    })
                }
                TaskEvent::TaskRated { task_id, agent_id } => {
                    serde_json::json!({
                        "type": "task_rated",
                        "task_id": task_id,
                        "agent_id": agent_id
                    })
                }
                TaskEvent::TaskScored { task_id, accepted } => {
                    serde_json::json!({
                        "type": "task_scored",
                        "task_id": task_id,
                        "accepted": accepted
                    })
                }
            };

            Ok::<_, std::convert::Infallible>(Event::default().data(data.to_string()).event(
                match event {
                    TaskEvent::TaskCreated { .. } => "task_created",
                    TaskEvent::TaskRated { .. } => "task_rated",
                    TaskEvent::TaskScored { .. } => "task_scored",
                },
            ))
        })
    });

    Sse::new(event_stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(30))
            .text("heartbeat"),
    )
}
