use eyre::{Result, WrapErr as _};
use protocol::TaskPhase;
use tracing::{error, info};

use crate::cli::AgentRole;
use crate::llm::{LlmClient, Message};

pub(crate) async fn poll_once(
    agent_id: &str,
    role: &AgentRole,
    llm: &LlmClient,
    coordinator: &client::CoordinatorClient,
) -> Result<()> {
    let tasks = coordinator
        .list_tasks()
        .await
        .wrap_err("failed to list tasks")?;

    for task in tasks {
        match (role, &task.phase) {
            (AgentRole::Worker, TaskPhase::AwaitingWork) => {
                handle_work(agent_id, llm, coordinator, &task.task.id, &task.task.prompt).await;
            }
            (
                AgentRole::Rater,
                TaskPhase::AwaitingRatings {
                    worker_output,
                    ratings,
                    ..
                },
            ) => {
                let already_rated = ratings.iter().any(|r| r.agent_id == agent_id);
                if !already_rated {
                    handle_rate(
                        agent_id,
                        llm,
                        coordinator,
                        &task.task.id,
                        &task.task.prompt,
                        worker_output,
                    )
                    .await;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn handle_work(
    agent_id: &str,
    llm: &LlmClient,
    coordinator: &client::CoordinatorClient,
    task_id: &uuid::Uuid,
    prompt: &str,
) {
    info!("working on task {task_id}");

    let messages = vec![Message {
        role: "user".into(),
        content: prompt.to_string(),
    }];

    let output = match llm.chat(&messages).await {
        Ok(o) => o,
        Err(e) => {
            error!("llm error for task {task_id}: {e}");
            return;
        }
    };

    match coordinator.submit_result(*task_id, agent_id, &output).await {
        Ok(_) => info!("submitted result for task {task_id}"),
        Err(e) => error!("submit result failed for {task_id}: {e}"),
    }
}

async fn handle_rate(
    agent_id: &str,
    llm: &LlmClient,
    coordinator: &client::CoordinatorClient,
    task_id: &uuid::Uuid,
    prompt: &str,
    worker_output: &str,
) {
    info!("rating task {task_id}");

    let rating_prompt = format!(
        "You are evaluating the quality of an AI agent's output.\n\
         \n\
         Task: {prompt}\n\
         Output: {worker_output}\n\
         \n\
         Rate this as GOOD or BAD. Then predict what percentage of other \
         raters will say GOOD (0.0 to 1.0).\n\
         \n\
         Respond ONLY as JSON: {{\"signal\": true, \"prediction\": 0.75}}\n\
         where signal is true for GOOD, false for BAD.",
    );

    let messages = vec![Message {
        role: "user".into(),
        content: rating_prompt,
    }];

    let raw = match llm.chat(&messages).await {
        Ok(r) => r,
        Err(e) => {
            error!("llm error rating task {task_id}: {e}");
            return;
        }
    };

    let (signal, prediction) = parse_rating(&raw).unwrap_or((false, 0.5));

    match coordinator
        .submit_rating(*task_id, agent_id, signal, prediction)
        .await
    {
        Ok(_) => info!("submitted rating for task {task_id}"),
        Err(e) => error!("submit rating failed for {task_id}: {e}"),
    }
}

fn parse_rating(raw: &str) -> Option<(bool, f64)> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')? + 1;
    let json_str = &raw[start..end];

    #[derive(serde::Deserialize)]
    struct LlmRating {
        signal: bool,
        prediction: f64,
    }

    let parsed: LlmRating = serde_json::from_str(json_str).ok()?;
    Some((parsed.signal, parsed.prediction.clamp(0.0, 1.0)))
}
