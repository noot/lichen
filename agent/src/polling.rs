use eyre::{Result, WrapErr as _};
use protocol::{SubmitRatingRequest, SubmitResultRequest, TaskPhase, TaskStatus};
use tracing::{error, info};

use crate::cli::AgentRole;
use crate::llm::Message;
use crate::Agent;

pub(crate) async fn poll_once(agent: &Agent) -> Result<()> {
    let url = format!("{}/tasks", agent.coordinator_url);
    let resp = agent
        .client
        .get(&url)
        .send()
        .await
        .wrap_err("failed to fetch tasks")?;
    let tasks: Vec<TaskStatus> = resp.json().await.wrap_err("failed to parse tasks")?;

    for task in tasks {
        match (&agent.role, &task.phase) {
            (AgentRole::Worker, TaskPhase::AwaitingWork) => {
                handle_work(agent, &task).await;
            }
            (AgentRole::Rater, TaskPhase::AwaitingRatings { ratings, .. }) => {
                let already_rated = ratings.iter().any(|r| r.agent_id == agent.agent_id);
                if !already_rated {
                    handle_rate(agent, &task).await;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn handle_work(agent: &Agent, task: &TaskStatus) {
    info!("working on task {}", task.task.id);

    let messages = vec![Message {
        role: "user".into(),
        content: task.task.prompt.clone(),
    }];

    let output = match agent.llm.chat(&messages).await {
        Ok(o) => o,
        Err(e) => {
            error!("llm error for task {}: {e}", task.task.id);
            return;
        }
    };

    let req = SubmitResultRequest {
        task_id: task.task.id,
        agent_id: agent.agent_id.clone(),
        output,
    };

    let url = format!("{}/tasks/{}/result", agent.coordinator_url, task.task.id);
    match agent.client.post(&url).json(&req).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("submitted result for task {}", task.task.id);
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("submit result failed for {}: {status} {body}", task.task.id);
        }
        Err(e) => error!("submit result error for {}: {e}", task.task.id),
    }
}

async fn handle_rate(agent: &Agent, task: &TaskStatus) {
    let TaskPhase::AwaitingRatings { worker_output, .. } = &task.phase else {
        return;
    };

    info!("rating task {}", task.task.id);

    let prompt = format!(
        "You are evaluating the quality of an AI agent's output.\n\
         \n\
         Task: {}\n\
         Output: {}\n\
         \n\
         Rate this as GOOD or BAD. Then predict what percentage of other \
         raters will say GOOD (0.0 to 1.0).\n\
         \n\
         Respond ONLY as JSON: {{\"signal\": true, \"prediction\": 0.75}}\n\
         where signal is true for GOOD, false for BAD.",
        task.task.prompt, worker_output
    );

    let messages = vec![Message {
        role: "user".into(),
        content: prompt,
    }];

    let raw = match agent.llm.chat(&messages).await {
        Ok(r) => r,
        Err(e) => {
            error!("llm error rating task {}: {e}", task.task.id);
            return;
        }
    };

    let (signal, prediction) = parse_rating(&raw).unwrap_or((false, 0.5));

    let req = SubmitRatingRequest {
        task_id: task.task.id,
        agent_id: agent.agent_id.clone(),
        signal,
        prediction,
    };

    let url = format!("{}/tasks/{}/rating", agent.coordinator_url, task.task.id);
    match agent.client.post(&url).json(&req).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("submitted rating for task {}", task.task.id);
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            error!("submit rating failed for {}: {status} {body}", task.task.id);
        }
        Err(e) => error!("submit rating error for {}: {e}", task.task.id),
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
