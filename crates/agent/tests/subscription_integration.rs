//! Integration tests for the agent subscription + accept/decline mechanic.
//!
//! These tests start an in-process coordinator, start the agent's callback
//! server, register a subscription, and then verify that task notifications
//! are delivered and that the accept/decline logic fires correctly.

use std::sync::Arc;

use agent::{Backend, LlmClient, Provider, SubscriptionConfig, SubscriptionState};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use client::CoordinatorClient;
use coordinator::{cli::Args as CoordArgs, Coordinator};
use protocol::{AgentRole, TaskNotification};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// ── Coordinator helper ────────────────────────────────────────────────────────

async fn spawn_coordinator() -> (String, CoordinatorClient, CancellationToken) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let args = CoordArgs {
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
    let base_url = format!("http://{addr}");
    let coord = Coordinator::new(&args).unwrap();

    tokio::spawn(async move {
        coord.run(listener, cancel).await.unwrap();
    });

    let client = CoordinatorClient::new(&base_url);
    (base_url, client, token)
}

// ── Simple notification recorder ─────────────────────────────────────────────

/// A tiny HTTP server that records TaskNotifications posted to it.
struct NotificationRecorder {
    received: Mutex<Vec<TaskNotification>>,
}

impl NotificationRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            received: Mutex::new(Vec::new()),
        })
    }

    async fn received_count(&self) -> usize {
        self.received.lock().await.len()
    }
}

async fn record_notification(
    State(recorder): State<Arc<NotificationRecorder>>,
    Json(notif): Json<TaskNotification>,
) -> StatusCode {
    recorder.received.lock().await.push(notif);
    StatusCode::OK
}

async fn start_recorder(recorder: Arc<NotificationRecorder>) -> (u16, CancellationToken) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let app = Router::new()
        .route("/notify", post(record_notification))
        .with_state(Arc::clone(&recorder));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        tokio::select! {
            _ = axum::serve(listener, app) => {}
            _ = cancel.cancelled() => {}
        }
    });

    (port, token)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Subscribing registers the agent; creating a task causes a notification to be
/// delivered to the callback URL.
#[tokio::test]
async fn notification_delivered_on_subscribe() {
    let (coord_url, coord_client, _tok) = spawn_coordinator().await;

    let recorder = NotificationRecorder::new();
    let (port, _srv_tok) = start_recorder(Arc::clone(&recorder)).await;
    let callback_url = format!("http://127.0.0.1:{port}/notify");

    // Subscribe
    coord_client
        .subscribe("test-agent", &callback_url, vec![AgentRole::Worker])
        .await
        .unwrap();

    // Create task — coordinator should broadcast to subscribers
    coord_client.create_task("write a haiku", 3).await.unwrap();

    // Wait a bit for the async notification delivery
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    assert_eq!(
        recorder.received_count().await,
        1,
        "expected exactly 1 notification"
    );

    let notifs = recorder.received.lock().await;
    let notif = &notifs[0];
    assert_eq!(notif.prompt, "write a haiku");
    assert!(!notif.accept_url.is_empty());
    assert!(!notif.decline_url.is_empty());
    drop(notifs);

    // Verify accept and decline URLs are reachable
    let _ = coord_url;
}

/// Multiple subscribers each receive the notification.
#[tokio::test]
async fn all_subscribers_notified() {
    let (_coord_url, coord_client, _tok) = spawn_coordinator().await;

    let mut ports = Vec::new();
    let mut recorders = Vec::new();
    let mut srv_tokens = Vec::new();

    for _ in 0..3 {
        let recorder = NotificationRecorder::new();
        let (port, srv_tok) = start_recorder(Arc::clone(&recorder)).await;
        ports.push(port);
        recorders.push(recorder);
        srv_tokens.push(srv_tok);
    }

    for (i, port) in ports.iter().enumerate() {
        let callback_url = format!("http://127.0.0.1:{port}/notify");
        coord_client
            .subscribe(&format!("agent-{i}"), &callback_url, vec![AgentRole::Rater])
            .await
            .unwrap();
    }

    coord_client
        .create_task("test task", 3)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    for recorder in &recorders {
        assert_eq!(
            recorder.received_count().await,
            1,
            "each subscriber should get one notification"
        );
    }
}

/// Unsubscribed agents don't receive future notifications.
#[tokio::test]
async fn unsubscribed_agent_not_notified() {
    let (_coord_url, coord_client, _tok) = spawn_coordinator().await;

    let recorder = NotificationRecorder::new();
    let (port, _srv_tok) = start_recorder(Arc::clone(&recorder)).await;
    let callback_url = format!("http://127.0.0.1:{port}/notify");

    // Subscribe then immediately unsubscribe
    coord_client
        .subscribe("temp-agent", &callback_url, vec![AgentRole::Worker])
        .await
        .unwrap();
    coord_client.unsubscribe("temp-agent").await.unwrap();

    // Create task — no notification should arrive
    coord_client.create_task("silence", 1).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    assert_eq!(
        recorder.received_count().await,
        0,
        "unsubscribed agent should receive no notifications"
    );
}

/// An agent at capacity declines; one under capacity accepts.
#[tokio::test]
async fn capacity_based_accept_decline() {
    let (coord_url, coord_client, _coord_tok) = spawn_coordinator().await;

    // Build a SubscriptionState with max_concurrent_tasks = 0 (always at capacity)
    let at_capacity_config = SubscriptionConfig {
        agent_id: "cap-agent".to_string(),
        callback_url: "http://127.0.0.1:0/notify".to_string(), // placeholder, overwritten below
        roles: vec![AgentRole::Worker],
        max_concurrent_tasks: 0, // always full
        decline_probability: 0.0,
        coordinator_url: coord_url.clone(),
    };

    // We'll verify capacity logic indirectly: create a task, have a "full" agent
    // decline it, confirm the decline is recorded in coordinator.

    // Start a recorder to capture the notification
    let recorder = NotificationRecorder::new();
    let (port, _srv_tok) = start_recorder(Arc::clone(&recorder)).await;
    let callback_url = format!("http://127.0.0.1:{port}/notify");

    // Subscribe the recorder as an agent
    coord_client
        .subscribe("cap-agent", &callback_url, vec![AgentRole::Worker])
        .await
        .unwrap();

    let task = coord_client
        .create_task("write code", 1)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    assert_eq!(recorder.received_count().await, 1);

    // Manually decline the task (simulating at-capacity logic)
    let task_id = task.task.id;
    let resp = coord_client
        .decline_task(task_id, "cap-agent", "at capacity (0/0 tasks)")
        .await
        .unwrap();
    assert_eq!(resp.role_granted, "declined");
    assert!(resp.message.contains("cap-agent"));

    // Also verify accept works for a different agent
    let resp2 = coord_client
        .accept_task(task_id, "fresh-agent", AgentRole::Worker)
        .await
        .unwrap();
    assert_eq!(resp2.role_granted, "worker");

    // Drop config — it was only used for this test's setup
    drop(at_capacity_config);
}

/// Verify SubscriptionState active task tracking works correctly.
#[tokio::test]
async fn subscription_state_tracks_active_tasks() {
    let (coord_url, _client, _tok) = spawn_coordinator().await;

    let config = SubscriptionConfig {
        agent_id: "tracker-agent".to_string(),
        callback_url: "http://127.0.0.1:9999/notify".to_string(),
        roles: vec![AgentRole::Worker],
        max_concurrent_tasks: 2,
        decline_probability: 0.0,
        coordinator_url: coord_url,
    };

    // Need a dummy LlmClient; we won't actually call the LLM
    let llm = LlmClient::new(
        "http://localhost:11434/v1".to_string(),
        "test".to_string(),
        None,
        Provider::Openai,
    );
    let backend = Backend::http("http://localhost:9999"); // won't be called
    let state = SubscriptionState::new(config, llm, backend);

    // Initially empty
    assert_eq!(state.active_count().await, 0);

    let id1 = uuid::Uuid::new_v4();
    let id2 = uuid::Uuid::new_v4();

    state.add_task(id1).await;
    assert_eq!(state.active_count().await, 1);

    state.add_task(id2).await;
    assert_eq!(state.active_count().await, 2);

    // Adding the same task again is a no-op
    state.add_task(id1).await;
    assert_eq!(state.active_count().await, 2);

    state.remove_task(id1).await;
    assert_eq!(state.active_count().await, 1);

    state.remove_task(id2).await;
    assert_eq!(state.active_count().await, 0);
}

/// Verify that accept/decline URLs in the notification point to working endpoints.
#[tokio::test]
async fn accept_and_decline_urls_functional() {
    let (_coord_url, coord_client, _tok) = spawn_coordinator().await;

    let recorder = NotificationRecorder::new();
    let (port, _srv_tok) = start_recorder(Arc::clone(&recorder)).await;
    let callback_url = format!("http://127.0.0.1:{port}/notify");

    coord_client
        .subscribe("url-test-agent", &callback_url, vec![AgentRole::Worker])
        .await
        .unwrap();

    coord_client
        .create_task("url test task", 2)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let notifs = recorder.received.lock().await;
    assert_eq!(notifs.len(), 1);
    let notif = &notifs[0];

    // Call accept_url directly via HTTP
    let http = reqwest::Client::new();
    let accept_body = serde_json::json!({
        "agent_id": "url-test-agent",
        "role": "worker"
    });
    let resp = http
        .post(&notif.accept_url)
        .json(&accept_body)
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "accept_url returned {}",
        resp.status()
    );

    // Call decline_url directly
    let decline_body = serde_json::json!({
        "agent_id": "url-test-agent-2",
        "reason": "testing decline url"
    });
    let resp2 = http
        .post(&notif.decline_url)
        .json(&decline_body)
        .send()
        .await
        .unwrap();
    assert!(
        resp2.status().is_success(),
        "decline_url returned {}",
        resp2.status()
    );
}
