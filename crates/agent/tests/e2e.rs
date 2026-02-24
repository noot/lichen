use agent::{Agent, AgentRole, Args, Provider};
use client::CoordinatorClient;
use coordinator::{cli::Args as CoordinatorArgs, Coordinator};
use protocol::TaskPhase;
use std::sync::Once;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter("agent=debug,coordinator=debug,client=debug")
            .with_test_writer()
            .init();
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct LlmConfig {
    api_key: String,
    base_url: String,
    model: String,
    provider: Provider,
}

fn llm_config() -> LlmConfig {
    let api_key = std::env::var("LLM_API_KEY").expect("set LLM_API_KEY to run this test");
    let base_url =
        std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com/v1".into());
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "claude-haiku-4-5-20251001".into());
    let provider = match std::env::var("LLM_PROVIDER").as_deref() {
        Ok("openai") => Provider::Openai,
        _ => Provider::Anthropic,
    };
    LlmConfig {
        api_key,
        base_url,
        model,
        provider,
    }
}

async fn spawn_coordinator() -> (String, CoordinatorClient, CancellationToken) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let args = CoordinatorArgs {
        port: 0,
        alpha: 1.0,
        beta: 1.0,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let coord = Coordinator::new(&args);

    tokio::spawn(async move {
        coord.run(listener, cancel).await.unwrap();
    });

    let base = format!("http://{addr}");
    let client = CoordinatorClient::new(&base);
    (base, client, token)
}

fn spawn_agent(
    coordinator_url: &str,
    role: AgentRole,
    agent_id: &str,
    token: CancellationToken,
    config: &LlmConfig,
) {
    spawn_agent_with_model(
        coordinator_url,
        role,
        agent_id,
        token,
        config,
        &config.model,
    );
}

fn spawn_agent_with_model(
    coordinator_url: &str,
    role: AgentRole,
    agent_id: &str,
    token: CancellationToken,
    config: &LlmConfig,
    model: &str,
) {
    let args = Args {
        port: 0,
        agent_id: agent_id.to_string(),
        model: model.to_string(),
        llm_url: config.base_url.clone(),
        api_key: Some(config.api_key.clone()),
        provider: config.provider,
        coordinator_url: coordinator_url.to_string(),
        role,
        poll_interval: 1,
    };

    let agent = Agent::new(args);
    tokio::spawn(async move {
        if let Err(e) = agent.run(token).await {
            eprintln!("agent error: {e}");
        }
    });
}

async fn poll_until_phase(
    client: &CoordinatorClient,
    task_id: Uuid,
    check: impl Fn(&TaskPhase) -> bool,
    timeout_secs: u64,
) -> protocol::TaskStatus {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    loop {
        if tokio::time::Instant::now() > deadline {
            let status = client.get_task(task_id).await.unwrap();
            panic!(
                "timed out after {timeout_secs}s; last phase: {:?}",
                status.phase
            );
        }
        if let Ok(status) = client.get_task(task_id).await {
            if check(&status.phase) {
                return status;
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn worker_submits_result() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    spawn_agent(
        &base,
        AgentRole::Worker,
        "worker-1",
        agent_token.clone(),
        &config,
    );

    let task = client
        .create_task("write a haiku about rust programming", 3)
        .await
        .unwrap();

    let status = poll_until_phase(
        &client,
        task.task.id,
        |p| matches!(p, TaskPhase::AwaitingRatings { .. }),
        30,
    )
    .await;

    match &status.phase {
        TaskPhase::AwaitingRatings {
            worker_id,
            worker_output,
            ..
        } => {
            assert_eq!(worker_id, "worker-1");
            assert!(
                !worker_output.is_empty(),
                "worker output should not be empty"
            );
        }
        other => panic!("expected AwaitingRatings, got {other:?}"),
    }

    agent_token.cancel();
    coord_token.cancel();
}

#[tokio::test]
#[ignore]
async fn full_lifecycle() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    spawn_agent(
        &base,
        AgentRole::Worker,
        "worker-1",
        agent_token.clone(),
        &config,
    );
    spawn_agent(
        &base,
        AgentRole::Rater,
        "rater-1",
        agent_token.clone(),
        &config,
    );
    spawn_agent(
        &base,
        AgentRole::Rater,
        "rater-2",
        agent_token.clone(),
        &config,
    );
    spawn_agent(
        &base,
        AgentRole::Rater,
        "rater-3",
        agent_token.clone(),
        &config,
    );

    let task = client
        .create_task("write a haiku about rust programming", 3)
        .await
        .unwrap();

    let status = poll_until_phase(
        &client,
        task.task.id,
        |p| matches!(p, TaskPhase::Scored { .. }),
        120,
    )
    .await;

    match &status.phase {
        TaskPhase::Scored {
            worker_id,
            ratings,
            scores,
            ..
        } => {
            assert_eq!(worker_id, "worker-1");
            assert_eq!(ratings.len(), 3, "expected 3 ratings");
            assert_eq!(scores.len(), 3, "expected 3 scores");
        }
        other => panic!("expected Scored, got {other:?}"),
    }

    agent_token.cancel();
    coord_token.cancel();
}

#[tokio::test]
#[ignore]
async fn multi_model_lifecycle() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    // different model per agent
    let models = [
        ("worker-sonnet", AgentRole::Worker, "claude-sonnet-4-6"),
        ("rater-gpt", AgentRole::Rater, "gpt-4o-mini"),
        ("rater-haiku", AgentRole::Rater, "claude-haiku-4-5"),
        ("rater-gemini", AgentRole::Rater, "gemini-2.5-flash"),
        ("rater-gpt5", AgentRole::Rater, "gpt-5-mini"),
        ("rater-llama", AgentRole::Rater, "llama-4-scout-17b-16e"),
    ];

    for (id, role, model) in &models {
        spawn_agent_with_model(&base, *role, id, agent_token.clone(), &config, model);
    }

    let task = client
        .create_task(
            "write a short function in rust that reverses a string. include a docstring.",
            5,
        )
        .await
        .unwrap();

    let status = poll_until_phase(
        &client,
        task.task.id,
        |p| matches!(p, TaskPhase::Scored { .. }),
        180,
    )
    .await;

    match &status.phase {
        TaskPhase::Scored {
            worker_id,
            worker_output,
            ratings,
            scores,
            bts_accepted,
            approval,
            accepted,
        } => {
            assert_eq!(worker_id, "worker-sonnet");
            assert!(!worker_output.is_empty());
            assert_eq!(ratings.len(), 5, "expected 5 ratings");
            assert_eq!(scores.len(), 5, "expected 5 scores");

            println!("\n=== multi-model results ===");
            println!("worker output:\n{worker_output}\n");
            println!(
                "accepted: {accepted} (approval: {approval:.2}, bts_accepted: {bts_accepted})"
            );
            for (rating, score) in ratings.iter().zip(scores.iter()) {
                let vote = if rating.signal { "good" } else { "bad" };
                println!(
                    "  {} — vote: {}, prediction: {:.2}, payment: {:.4}",
                    score.agent_id, vote, rating.prediction, score.payment
                );
            }
        }
        other => panic!("expected Scored, got {other:?}"),
    }

    agent_token.cancel();
    coord_token.cancel();
}

#[tokio::test]
#[ignore]
async fn ten_model_lifecycle() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    // 1 worker + 9 raters across different providers and model sizes
    let models = [
        ("worker-sonnet", AgentRole::Worker, "claude-sonnet-4-6"),
        ("rater-haiku", AgentRole::Rater, "claude-haiku-4-5"),
        ("rater-sonnet-4.5", AgentRole::Rater, "claude-sonnet-4-5"),
        ("rater-gpt5", AgentRole::Rater, "gpt-5"),
        ("rater-gpt4.1m", AgentRole::Rater, "gpt-4.1-mini"),
        ("rater-gpt5m", AgentRole::Rater, "gpt-5-mini"),
        ("rater-gemini", AgentRole::Rater, "gemini-2.5-flash"),
        ("rater-gemini3", AgentRole::Rater, "gemini-3-pro"),
        ("rater-llama", AgentRole::Rater, "llama-4-maverick-17b-128e"),
        ("rater-gpt4o", AgentRole::Rater, "gpt-4o"),
    ];

    for (id, role, model) in &models {
        spawn_agent_with_model(&base, *role, id, agent_token.clone(), &config, model);
    }

    let task = client
        .create_task(
            "write a short function in rust that reverses a string. include a docstring.",
            9,
        )
        .await
        .unwrap();

    let status = poll_until_phase(
        &client,
        task.task.id,
        |p| matches!(p, TaskPhase::Scored { .. }),
        300,
    )
    .await;

    match &status.phase {
        TaskPhase::Scored {
            worker_id,
            worker_output,
            ratings,
            scores,
            bts_accepted,
            approval,
            accepted,
        } => {
            assert_eq!(worker_id, "worker-sonnet");
            assert!(!worker_output.is_empty());
            assert_eq!(ratings.len(), 9, "expected 9 ratings");
            assert_eq!(scores.len(), 9, "expected 9 scores");

            println!("\n=== 10-model results ===");
            println!("worker output:\n{worker_output}\n");
            println!(
                "accepted: {accepted} (approval: {approval:.2}, bts_accepted: {bts_accepted})"
            );
            for (rating, score) in ratings.iter().zip(scores.iter()) {
                let vote = if rating.signal { "good" } else { "bad" };
                println!(
                    "  {:20} — vote: {:4}, prediction: {:.2}, payment: {:.4}",
                    score.agent_id, vote, rating.prediction, score.payment
                );
            }
        }
        other => panic!("expected Scored, got {other:?}"),
    }

    agent_token.cancel();
    coord_token.cancel();
}
