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
        onchain: false,
        rpc_url: None,
        contract_address: None,
        private_key: None,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let coord = Coordinator::new(&args).unwrap();

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
        backend: agent::BackendMode::Http,
        contract_address: None,
        rpc_url: "http://localhost:8545".to_string(),
        role,
        poll_interval: 1,
    };

    let agent = Agent::new(args).expect("failed to create agent");
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

    let task = client.create_task("TASK_PLACEHOLDER", 3).await.unwrap();

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
        ("rater-o4m", AgentRole::Rater, "o4-mini"),
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
        ("worker-gpt5", AgentRole::Worker, "gpt-5"),
        ("rater-haiku", AgentRole::Rater, "claude-haiku-4-5"),
        ("rater-sonnet-4.5", AgentRole::Rater, "claude-sonnet-4-5"),
        ("rater-gpt5", AgentRole::Rater, "gpt-5"),
        ("rater-gpt4.1m", AgentRole::Rater, "gpt-4.1-mini"),
        ("rater-gpt5m", AgentRole::Rater, "gpt-5-mini"),
        ("rater-gemini", AgentRole::Rater, "gemini-2.5-flash"),
        ("rater-gemini3", AgentRole::Rater, "gemini-3-pro"),
        ("rater-o4m", AgentRole::Rater, "o4-mini"),
        ("rater-gpt4o", AgentRole::Rater, "gpt-4o"),
    ];

    for (id, role, model) in &models {
        spawn_agent_with_model(&base, *role, id, agent_token.clone(), &config, model);
    }

    let task = client.create_task("TASK_PLACEHOLDER", 9).await.unwrap();

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
            assert_eq!(worker_id, "worker-gpt5");
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

#[tokio::test]
#[ignore]
async fn bad_worker_rejected() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    // only raters — we'll submit garbage work directly via the client
    let raters = [
        ("rater-haiku", "claude-haiku-4-5"),
        ("rater-sonnet-4.5", "claude-sonnet-4-5"),
        ("rater-gpt5", "gpt-5"),
        ("rater-gpt4.1m", "gpt-4.1-mini"),
        ("rater-gpt5m", "gpt-5-mini"),
        ("rater-gemini", "gemini-2.5-flash"),
        ("rater-gemini3", "gemini-3-pro"),
        ("rater-o4m", "o4-mini"),
        ("rater-gpt4o", "gpt-4o"),
    ];

    for (id, model) in &raters {
        spawn_agent_with_model(
            &base,
            AgentRole::Rater,
            id,
            agent_token.clone(),
            &config,
            model,
        );
    }

    let task = client
        .create_task(
            "write a rust function that sorts a vector of integers in ascending order. include a docstring.",
            9,
        )
        .await
        .unwrap();

    // submit intentionally terrible work
    let bad_output = "def sort_list(lst):\n    return lst.sort()\n\n# this is python not rust lol\n# also .sort() returns None";
    client
        .submit_result(task.task.id, "bad-worker", bad_output)
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
            assert_eq!(worker_id, "bad-worker");

            println!("\n=== bad worker results ===");
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

            // bad work should be rejected
            assert!(!accepted, "bad work should not be accepted");
        }
        other => panic!("expected Scored, got {other:?}"),
    }

    agent_token.cancel();
    coord_token.cancel();
}

#[tokio::test]
#[ignore]
async fn twentysix_model_lifecycle() {
    init_tracing();
    let config = llm_config();
    let (base, client, coord_token) = spawn_coordinator().await;
    let agent_token = coord_token.child_token();

    // 1 worker + 25 raters across all available providers
    let models = [
        // worker
        ("worker-opus45", AgentRole::Worker, "claude-opus-4-5"),
        // Claude raters (7)
        ("rater-haiku45", AgentRole::Rater, "claude-haiku-4-5"),
        ("rater-haiku35", AgentRole::Rater, "claude-3-5-haiku"),
        ("rater-sonnet45", AgentRole::Rater, "claude-sonnet-4-5"),
        ("rater-sonnet4", AgentRole::Rater, "claude-sonnet-4"),
        ("rater-sonnet37", AgentRole::Rater, "claude-3-7-sonnet"),
        ("rater-opus41", AgentRole::Rater, "claude-opus-4-1"),
        ("rater-sonnet46", AgentRole::Rater, "claude-sonnet-4-6"),
        // GPT raters (9)
        ("rater-gpt4o", AgentRole::Rater, "gpt-4o"),
        ("rater-gpt4om", AgentRole::Rater, "gpt-4o-mini"),
        ("rater-gpt41", AgentRole::Rater, "gpt-4.1"),
        ("rater-gpt41m", AgentRole::Rater, "gpt-4.1-mini"),
        ("rater-gpt41n", AgentRole::Rater, "gpt-4.1-nano"),
        ("rater-gpt5", AgentRole::Rater, "gpt-5"),
        ("rater-gpt5m", AgentRole::Rater, "gpt-5-mini"),
        ("rater-gpt5n", AgentRole::Rater, "gpt-5-nano"),
        ("rater-gpt52", AgentRole::Rater, "gpt-5.2"),
        // Gemini raters (6)
        ("rater-gem20f", AgentRole::Rater, "gemini-2.0-flash"),
        ("rater-gem25f", AgentRole::Rater, "gemini-2.5-flash"),
        ("rater-gem25p", AgentRole::Rater, "gemini-2.5-pro"),
        ("rater-gem3f", AgentRole::Rater, "gemini-3-flash"),
        ("rater-gem3p", AgentRole::Rater, "gemini-3-pro"),
        ("rater-gem31p", AgentRole::Rater, "gemini-3.1-pro"),
        // Reasoning raters (3)
        ("rater-o3", AgentRole::Rater, "o3"),
        ("rater-o3m", AgentRole::Rater, "o3-mini"),
        ("rater-o4m", AgentRole::Rater, "o4-mini"),
    ];

    for (id, role, model) in &models {
        spawn_agent_with_model(&base, *role, id, agent_token.clone(), &config, model);
    }

    let task = client
        .create_task(
            "implement a correct lock-free concurrent hash map in rust with insert, get, and remove operations. handle table resizing when load factor exceeds 0.75. use linear probing with proper memory ordering on all atomic ops. include unit tests with concurrent readers and writers.",
            25,
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
            assert_eq!(worker_id, "worker-opus45");
            assert!(!worker_output.is_empty());
            assert_eq!(ratings.len(), 25, "expected 25 ratings");
            assert_eq!(scores.len(), 25, "expected 25 scores");

            println!("\n=== 26-model results ===");
            println!("worker output:\n{worker_output}\n");
            println!(
                "accepted: {accepted} (approval: {approval:.2}, bts_accepted: {bts_accepted})"
            );
            for (rating, score) in ratings.iter().zip(scores.iter()) {
                let vote = if rating.signal { "good" } else { "bad " };
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
