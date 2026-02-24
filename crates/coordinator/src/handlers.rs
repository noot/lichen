use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use protocol::{
    CreateTaskRequest, SubmitRatingRequest, SubmitResultRequest, Task, TaskPhase, TaskStatus,
};
use tracing::info;
use uuid::Uuid;

use crate::coordinator::AppState;
use crate::scoring;

/// POST /tasks — create a new task
pub(crate) async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTaskRequest>,
) -> (StatusCode, Json<TaskStatus>) {
    let task_id = Uuid::new_v4();
    let status = TaskStatus {
        task: Task {
            id: task_id,
            prompt: req.prompt,
        },
        phase: TaskPhase::AwaitingWork,
        num_raters_required: req.num_raters,
    };

    state.tasks.write().await.insert(task_id, status.clone());
    info!("created task {task_id}");

    (StatusCode::CREATED, Json(status))
}

/// GET /tasks — list all tasks
pub(crate) async fn list_tasks(State(state): State<Arc<AppState>>) -> Json<Vec<TaskStatus>> {
    let tasks = state.tasks.read().await;
    Json(tasks.values().cloned().collect())
}

/// GET /tasks/:task_id — get task status
pub(crate) async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<TaskStatus>, StatusCode> {
    state
        .tasks
        .read()
        .await
        .get(&task_id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// POST /tasks/:task_id/result — worker pushes output
pub(crate) async fn submit_result(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<SubmitResultRequest>,
) -> Result<Json<TaskStatus>, (StatusCode, String)> {
    let mut tasks = state.tasks.write().await;
    let task = tasks
        .get_mut(&task_id)
        .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

    if !matches!(task.phase, TaskPhase::AwaitingWork) {
        return Err((StatusCode::CONFLICT, "task already has a result".into()));
    }

    task.phase = TaskPhase::AwaitingRatings {
        worker_id: req.agent_id.clone(),
        worker_output: req.output,
        ratings: vec![],
    };

    info!("task {task_id}: result submitted by {}", req.agent_id);
    Ok(Json(task.clone()))
}

/// POST /tasks/:task_id/rating — rater pushes rating
pub(crate) async fn submit_rating(
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
    Json(req): Json<SubmitRatingRequest>,
) -> Result<Json<TaskStatus>, (StatusCode, String)> {
    let mut tasks = state.tasks.write().await;
    let task = tasks
        .get_mut(&task_id)
        .ok_or((StatusCode::NOT_FOUND, "task not found".into()))?;

    // validate phase and check for duplicate raters
    match &task.phase {
        TaskPhase::AwaitingWork => {
            return Err((StatusCode::CONFLICT, "task has no result yet".into()));
        }
        TaskPhase::Scored { .. } => {
            return Err((StatusCode::CONFLICT, "task already scored".into()));
        }
        TaskPhase::AwaitingRatings { ratings, .. } => {
            if ratings.iter().any(|r| r.agent_id == req.agent_id) {
                return Err((StatusCode::CONFLICT, "agent already rated this task".into()));
            }
        }
    }

    // push rating and check if we should score
    let should_score = {
        let TaskPhase::AwaitingRatings { ratings, .. } = &mut task.phase else {
            unreachable!()
        };
        ratings.push(req.clone());
        info!(
            "task {task_id}: rating from {} ({}/{})",
            req.agent_id,
            ratings.len(),
            task.num_raters_required
        );
        ratings.len() >= task.num_raters_required
    };

    if should_score {
        let TaskPhase::AwaitingRatings {
            worker_id,
            worker_output,
            ratings,
        } = std::mem::replace(&mut task.phase, TaskPhase::AwaitingWork)
        else {
            unreachable!()
        };

        let scores = scoring::rbts_score(&ratings, state.alpha, state.beta);
        let actual_good = ratings.iter().filter(|r| r.signal).count() as f64 / ratings.len() as f64;
        let predicted_good =
            ratings.iter().map(|r| r.prediction).sum::<f64>() / ratings.len() as f64;
        let bts_accepted = actual_good > predicted_good;

        info!("task {task_id}: scored (bts_accepted={bts_accepted}) — {scores:?}");
        task.phase = TaskPhase::Scored {
            worker_id,
            worker_output,
            ratings,
            scores,
            bts_accepted,
        };
    }

    Ok(Json(task.clone()))
}
