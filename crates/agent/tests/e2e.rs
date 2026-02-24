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
    let args = Args {
        port: 0,
        agent_id: agent_id.to_string(),
        model: config.model.clone(),
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
