use client::CoordinatorClient;
use coordinator::{cli::Args, Coordinator};
use protocol::TaskPhase;
use reqwest::StatusCode;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// spin up a coordinator on a random port, return the client and cancel token.
async fn spawn_coordinator() -> (CoordinatorClient, CancellationToken) {
    let token = CancellationToken::new();
    let cancel = token.clone();

    let args = Args {
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
    (CoordinatorClient::new(&base), token)
}

#[tokio::test]
async fn full_task_lifecycle() {
    let (c, _tok) = spawn_coordinator().await;

    // create task
    let task = c.create_task("write a haiku", 3).await.unwrap();
    assert!(matches!(task.phase, TaskPhase::AwaitingWork));

    // submit work
    let task = c
        .submit_result(task.task.id, "worker1", "an old silent pond")
        .await
        .unwrap();
    assert!(matches!(task.phase, TaskPhase::AwaitingRatings { .. }));

    // submit ratings — 2 good, 1 bad. both good raters underpredict good fraction.
    c.submit_rating(task.task.id, "rater1", true, 0.4)
        .await
        .unwrap();
    c.submit_rating(task.task.id, "rater2", true, 0.3)
        .await
        .unwrap();
    let task = c
        .submit_rating(task.task.id, "rater3", false, 0.5)
        .await
        .unwrap();

    match &task.phase {
        TaskPhase::Scored {
            bts_accepted,
            scores,
            ..
        } => {
            // actual good = 2/3 ≈ 0.67, avg predicted = 0.4. good is surprisingly popular.
            assert!(bts_accepted);
            assert_eq!(scores.len(), 3);
        }
        other => panic!("expected Scored, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_work_rejected_by_bts() {
    let (c, _tok) = spawn_coordinator().await;
    let task = c.create_task("solve 2+2", 3).await.unwrap();
    c.submit_result(task.task.id, "worker1", "5").await.unwrap();

    // all raters say bad, and they predicted most would say bad
    c.submit_rating(task.task.id, "rater1", false, 0.2)
        .await
        .unwrap();
    c.submit_rating(task.task.id, "rater2", false, 0.1)
        .await
        .unwrap();
    let task = c
        .submit_rating(task.task.id, "rater3", false, 0.3)
        .await
        .unwrap();

    match &task.phase {
        TaskPhase::Scored { bts_accepted, .. } => {
            assert!(!bts_accepted);
        }
        other => panic!("expected Scored, got {other:?}"),
    }
}

#[tokio::test]
async fn liar_scores_lowest() {
    let (c, _tok) = spawn_coordinator().await;
    let task = c.create_task("check this work", 4).await.unwrap();
    c.submit_result(task.task.id, "worker1", "good output")
        .await
        .unwrap();

    // 3 honest raters say good, 1 liar says bad
    c.submit_rating(task.task.id, "honest1", true, 0.5)
        .await
        .unwrap();
    c.submit_rating(task.task.id, "honest2", true, 0.5)
        .await
        .unwrap();
    c.submit_rating(task.task.id, "honest3", true, 0.4)
        .await
        .unwrap();
    let task = c
        .submit_rating(task.task.id, "liar", false, 0.8)
        .await
        .unwrap();

    match &task.phase {
        TaskPhase::Scored { scores, .. } => {
            let liar_score = scores
                .iter()
                .find(|s| s.agent_id == "liar")
                .unwrap()
                .payment;
            let honest_min = scores
                .iter()
                .filter(|s| s.agent_id != "liar")
                .map(|s| s.payment)
                .fold(f64::INFINITY, f64::min);
            assert!(
                liar_score < honest_min,
                "liar ({liar_score}) should score lower than all honest agents ({honest_min})"
            );
        }
        other => panic!("expected Scored, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_rating_rejected() {
    let (c, _tok) = spawn_coordinator().await;
    let task = c.create_task("test task", 3).await.unwrap();
    c.submit_result(task.task.id, "worker1", "output")
        .await
        .unwrap();
    c.submit_rating(task.task.id, "rater1", true, 0.5)
        .await
        .unwrap();

    let (status, _) = c
        .submit_rating_raw(task.task.id, "rater1", false, 0.3)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn rating_before_result_rejected() {
    let (c, _tok) = spawn_coordinator().await;
    let task = c.create_task("test task", 3).await.unwrap();

    let (status, _) = c
        .submit_rating_raw(task.task.id, "rater1", true, 0.5)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn double_result_rejected() {
    let (c, _tok) = spawn_coordinator().await;
    let task = c.create_task("test task", 3).await.unwrap();
    c.submit_result(task.task.id, "worker1", "first")
        .await
        .unwrap();

    let (status, _) = c
        .submit_result_raw(task.task.id, "worker2", "second")
        .await
        .unwrap();
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn get_nonexistent_task_returns_error() {
    let (c, _tok) = spawn_coordinator().await;
    let result = c.get_task(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn list_tasks_returns_all() {
    let (c, _tok) = spawn_coordinator().await;
    c.create_task("task 1", 3).await.unwrap();
    c.create_task("task 2", 3).await.unwrap();

    let tasks = c.list_tasks().await.unwrap();
    assert_eq!(tasks.len(), 2);
}
