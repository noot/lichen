//! HTTP notification helpers — fire-and-forget POSTs to agent callback URLs.

use tracing::{debug, warn};

use crate::coordinator::AgentSubscription;

static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn http() -> &'static reqwest::Client {
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("failed to build HTTP client")
    })
}

/// POST `notification` to every subscriber's `callback_url`.
/// Failures are logged but do not abort the loop.
/// Spawns a Tokio task per subscriber so callers don't wait for delivery.
pub(crate) fn broadcast_task_notification(
    subs: &[AgentSubscription],
    notification: &protocol::TaskNotification,
) {
    for sub in subs {
        let url = sub.callback_url.clone();
        let body = notification.clone();
        let agent_id = sub.agent_id.clone();

        tokio::spawn(async move {
            match http().post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    debug!("notified agent {agent_id} at {url}");
                }
                Ok(resp) => {
                    warn!(
                        "agent {agent_id} callback {url} returned {}",
                        resp.status()
                    );
                }
                Err(e) => {
                    warn!("agent {agent_id} callback {url} failed: {e}");
                }
            }
        });
    }
}
