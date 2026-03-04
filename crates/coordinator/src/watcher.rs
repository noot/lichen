//! On-chain event watcher for the open-marketplace coordinator.
//!
//! Polls the LichenCoordinator contract for new events and:
//! - Registers newly created tasks in the local dispatch table
//! - Notifies subscribed agents via their callback URLs
//! - Emits SSE [`TaskEvent`]s for browser/client consumers

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use eyre::Result;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::coordinator::{AppState, TaskDispatch, TaskEvent};
use crate::notifier;

/// How often to poll the chain for new events.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Run the event watcher loop.  Cancels cleanly when `token` fires.
pub(crate) async fn run_event_watcher(
    client: Arc<onchain::OnchainClient>,
    state: Arc<AppState>,
    token: tokio_util::sync::CancellationToken,
) -> Result<()> {
    info!("on-chain event watcher started");

    // Track which on-chain task IDs we've already seen so we don't double-notify.
    let mut seen_tasks: HashSet<u64> = HashSet::new();

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                info!("on-chain event watcher stopping");
                return Ok(());
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
        }

        match poll_once(&client, &state, &mut seen_tasks).await {
            Ok(()) => {}
            Err(e) => warn!("on-chain poll error: {e:#}"),
        }
    }
}

/// Single polling iteration — fetch active tasks, register new ones, notify agents.
async fn poll_once(
    client: &Arc<onchain::OnchainClient>,
    state: &Arc<AppState>,
    seen_tasks: &mut HashSet<u64>,
) -> Result<()> {
    let active_ids = client.get_active_tasks().await?;
    debug!("on-chain active tasks: {:?}", active_ids);

    for onchain_id in active_ids {
        if seen_tasks.contains(&onchain_id) {
            continue;
        }
        seen_tasks.insert(onchain_id);

        match handle_new_task(client, state, onchain_id).await {
            Ok(task_id) => {
                info!("on-chain task {onchain_id} → coordinator task {task_id}");
            }
            Err(e) => {
                warn!("failed to handle on-chain task {onchain_id}: {e:#}");
                // Remove from seen so we retry next poll
                seen_tasks.remove(&onchain_id);
            }
        }
    }

    Ok(())
}

/// Register a newly discovered on-chain task with the coordinator and notify agents.
async fn handle_new_task(
    client: &Arc<onchain::OnchainClient>,
    state: &Arc<AppState>,
    onchain_id: u64,
) -> Result<Uuid> {
    // Fetch task details from chain
    let (task, _ratings) = client.get_task(onchain_id).await?;

    // Allocate a coordinator-level UUID for this task
    let task_id = Uuid::new_v4();

    // Initialise dispatch state
    state
        .subscriptions
        .dispatches
        .write()
        .await
        .insert(task_id, TaskDispatch::default());

    // Emit SSE event
    let _ = state.event_tx.send(TaskEvent::TaskCreated {
        task_id,
        prompt: format!("onchain:{onchain_id}"),
    });

    // Notify subscribed agents
    let subs = state.subscriptions.list().await;
    if subs.is_empty() {
        debug!("no subscribed agents to notify for task {task_id}");
        return Ok(task_id);
    }

    let max_raters = task.maxRaters;
    let min_raters = task.minRaters;
    let deadline: u64 = task.deadline.try_into().unwrap_or(0);

    let notification = protocol::TaskNotification {
        task_id,
        // We only store the hash on-chain; expose the on-chain ID as the prompt.
        prompt: format!("onchain:{onchain_id}"),
        onchain_task_id: Some(onchain_id),
        max_raters,
        min_raters,
        deadline,
        accept_url: format!("{}/tasks/{task_id}/accept", state.base_url),
        decline_url: format!("{}/tasks/{task_id}/decline", state.base_url),
    };

    notifier::broadcast_task_notification(&subs, &notification);

    Ok(task_id)
}
