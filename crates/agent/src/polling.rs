use eyre::{Result, WrapErr as _};
use protocol::TaskPhase;
use tracing::{error, info};

use crate::backend::Backend;
use crate::cli::AgentRole;
use crate::llm::{LlmClient, Message};

pub(crate) async fn poll_once(
    agent_id: &str,
    role: &AgentRole,
    llm: &LlmClient,
    backend: &Backend,
) -> Result<()> {
    let tasks = backend
        .list_tasks()
        .await
        .wrap_err("failed to list tasks")?;

    for task in tasks {
        match (role, &task.phase) {
            (AgentRole::Worker, TaskPhase::AwaitingWork) => {
                handle_work(agent_id, llm, backend, &task.task.id, &task.task.prompt).await;
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
                        backend,
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

/// Public (within crate) entry point for subscription-based task dispatch.
pub(crate) async fn handle_work_notification(
    agent_id: &str,
    llm: &LlmClient,
    backend: &Backend,
    task_id: uuid::Uuid,
    prompt: &str,
) {
    handle_work(agent_id, llm, backend, &task_id, prompt).await;
}

/// Public (within crate) entry point for subscription-based rating dispatch.
pub(crate) async fn handle_rate_notification(
    agent_id: &str,
    llm: &LlmClient,
    backend: &Backend,
    task_id: uuid::Uuid,
    prompt: &str,
    worker_output: &str,
) {
    handle_rate(agent_id, llm, backend, &task_id, prompt, worker_output).await;
}

async fn handle_work(
    agent_id: &str,
    llm: &LlmClient,
    backend: &Backend,
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

    info!("agent {agent_id} completed task {task_id}: {output}");
    match backend.submit_result(*task_id, agent_id, &output).await {
        Ok(_) => info!("submitted result for task {task_id}"),
        Err(e) => error!("submit result failed for {task_id}: {e}"),
    }
}

async fn handle_rate(
    agent_id: &str,
    llm: &LlmClient,
    backend: &Backend,
    task_id: &uuid::Uuid,
    prompt: &str,
    worker_output: &str,
) {
    info!("rating task {task_id}");

    let rating_prompt = format!(
        "You are a rater in a decentralized quality assurance protocol. You have staked \
         collateral on this rating. Your payment depends on honesty and accuracy:\n\
         \n\
         - HONEST, ACCURATE ratings are REWARDED (you earn back your stake + bonus)\n\
         - DISHONEST or LAZY ratings are PUNISHED (you lose part or all of your stake)\n\
         \n\
         The scoring mechanism (Bayesian Truth Serum) can detect dishonesty by comparing \
         your vote and prediction against the group. Gaming the system will cost you money.\n\
         \n\
         Your job: evaluate the quality of the following AI-generated output for the given task.\n\
         \n\
         === TASK ===\n\
         {prompt}\n\
         \n\
         === OUTPUT ===\n\
         {worker_output}\n\
         \n\
         Rate this as GOOD or BAD based on correctness, completeness, and quality. \
         Then predict what fraction of other raters will say GOOD (0.0 to 1.0).\n\
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
    let vote = if signal { "good" } else { "bad" };
    info!("agent {agent_id} rated task {task_id}: {vote}, prediction={prediction:.2}");

    match backend
        .submit_rating(*task_id, agent_id, signal, prediction)
        .await
    {
        Ok(_) => info!("submitted rating for task {task_id}"),
        Err(e) => error!("submit rating failed for {task_id}: {e}"),
    }
}

fn parse_rating(raw: &str) -> Option<(bool, f64)> {
    let start = raw.find('{')?;
    #[allow(clippy::arithmetic_side_effects)]
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
