use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    routing::{delete, get, post},
    Router,
};
use eyre::{Result, WrapErr as _};
use protocol::AgentRole;
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::backend::{InMemoryBackend, OnchainBackend, TaskBackend};
use crate::cli::Args;
use crate::handlers;
use crate::watcher;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum TaskEvent {
    TaskCreated {
        task_id: Uuid,
        prompt: String,
    },
    TaskAccepted {
        task_id: Uuid,
        agent_id: String,
        role: AgentRole,
    },
    TaskDeclined {
        task_id: Uuid,
        agent_id: String,
    },
    TaskRated {
        task_id: Uuid,
        agent_id: String,
    },
    TaskScored {
        task_id: Uuid,
        accepted: bool,
    },
    TaskFinalized {
        task_id: Uuid,
    },
    TaskCancelled {
        task_id: Uuid,
    },
}

// ── Subscription registry ─────────────────────────────────────────────────────

/// A registered agent subscription.
#[derive(Debug, Clone)]
pub(crate) struct AgentSubscription {
    pub(crate) agent_id: String,
    pub(crate) callback_url: String,
    pub(crate) roles: Vec<AgentRole>,
}

/// Per-task accept/decline tracking for the open marketplace.
#[derive(Debug, Default)]
pub(crate) struct TaskDispatch {
    /// agent_id of the agent that accepted the worker role (first-come wins).
    pub(crate) worker: Option<String>,
    /// Agents that declined this task.
    pub(crate) declined: Vec<String>,
    /// Agents queued as raters (accepted with rater role).
    pub(crate) raters: Vec<String>,
}

pub(crate) struct SubscriptionRegistry {
    /// agent_id → subscription
    pub(crate) subscriptions: RwLock<HashMap<String, AgentSubscription>>,
    /// task_id → dispatch state
    pub(crate) dispatches: RwLock<HashMap<Uuid, TaskDispatch>>,
}

impl SubscriptionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            subscriptions: RwLock::new(HashMap::new()),
            dispatches: RwLock::new(HashMap::new()),
        }
    }

    /// Register or update an agent subscription.
    pub(crate) async fn subscribe(&self, sub: AgentSubscription) {
        let id = sub.agent_id.clone();
        self.subscriptions.write().await.insert(id.clone(), sub);
        info!("agent {id} subscribed");
    }

    /// Remove a subscription.
    pub(crate) async fn unsubscribe(&self, agent_id: &str) -> bool {
        let removed = self
            .subscriptions
            .write()
            .await
            .remove(agent_id)
            .is_some();
        if removed {
            info!("agent {agent_id} unsubscribed");
        }
        removed
    }

    /// Return a snapshot of all current subscriptions.
    pub(crate) async fn list(&self) -> Vec<AgentSubscription> {
        self.subscriptions.read().await.values().cloned().collect()
    }
}

// ── Shared application state ──────────────────────────────────────────────────

pub(crate) struct AppState {
    pub(crate) alpha: f64,
    pub(crate) beta: f64,
    pub(crate) backend: Arc<dyn TaskBackend>,
    pub(crate) event_tx: broadcast::Sender<TaskEvent>,
    pub(crate) subscriptions: Arc<SubscriptionRegistry>,
    /// Base URL of this coordinator instance (used to build accept/decline URLs
    /// in task notifications sent to agents).
    pub(crate) base_url: String,
    /// True when the coordinator is running with an on-chain backend.
    pub(crate) onchain: bool,
}

// ── Coordinator ───────────────────────────────────────────────────────────────

pub struct Coordinator {
    alpha: f64,
    beta: f64,
    backend: Arc<dyn TaskBackend>,
    onchain: bool,
    onchain_client: Option<Arc<onchain::OnchainClient>>,
}

impl Coordinator {
    pub fn new(args: &Args) -> Result<Self> {
        let (backend, onchain_client): (Arc<dyn TaskBackend>, Option<Arc<onchain::OnchainClient>>) =
            if args.onchain {
                let contract_address = args
                    .contract_address
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("contract_address required when --onchain is set"))?
                    .parse()
                    .wrap_err("invalid contract address")?;

                let client = Arc::new(
                    onchain::OnchainClient::new(
                        args.rpc_url
                            .as_ref()
                            .ok_or_else(|| eyre::eyre!("rpc_url required when --onchain is set"))?,
                        contract_address,
                        args.private_key.as_ref().ok_or_else(|| {
                            eyre::eyre!("private_key required when --onchain is set")
                        })?,
                    )
                    .wrap_err("failed to create on-chain client")?,
                );

                let backend = Arc::new(OnchainBackend::new(
                    onchain::OnchainClient::new(
                        args.rpc_url.as_ref().unwrap(),
                        contract_address,
                        args.private_key.as_ref().unwrap(),
                    )
                    .wrap_err("failed to create on-chain backend client")?,
                ));
                (backend, Some(client))
            } else {
                (Arc::new(InMemoryBackend::new()), None)
            };

        Ok(Self {
            alpha: args.alpha,
            beta: args.beta,
            backend,
            onchain: args.onchain,
            onchain_client,
        })
    }

    pub async fn run(
        self,
        listener: tokio::net::TcpListener,
        token: CancellationToken,
    ) -> Result<()> {
        let Self {
            alpha,
            beta,
            backend,
            onchain,
            onchain_client,
        } = self;

        let local_addr = listener.local_addr().wrap_err("failed to get local addr")?;
        let base_url = format!("http://{local_addr}");
        info!("coordinator listening on {base_url}");

        let (event_tx, _rx) = broadcast::channel(256);

        let subscriptions = Arc::new(SubscriptionRegistry::new());

        let state = Arc::new(AppState {
            alpha,
            beta,
            backend,
            event_tx: event_tx.clone(),
            subscriptions: Arc::clone(&subscriptions),
            base_url,
            onchain,
        });

        // Spawn on-chain event watcher if enabled
        if let Some(client) = onchain_client {
            let watcher_state = Arc::clone(&state);
            let watcher_token = token.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    watcher::run_event_watcher(client, watcher_state, watcher_token).await
                {
                    warn!("on-chain event watcher exited: {e:#}");
                }
            });
        }

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            // Task CRUD
            .route("/tasks", post(handlers::create_task))
            .route("/tasks", get(handlers::list_tasks))
            .route("/tasks/{task_id}", get(handlers::get_task))
            .route("/tasks/{task_id}/result", post(handlers::submit_result))
            .route("/tasks/{task_id}/rating", post(handlers::submit_rating))
            // Open-marketplace task lifecycle
            .route("/tasks/{task_id}/accept", post(handlers::accept_task))
            .route("/tasks/{task_id}/decline", post(handlers::decline_task))
            .route("/tasks/{task_id}/finalize", post(handlers::finalize_task))
            .route("/tasks/{task_id}/cancel", post(handlers::cancel_task))
            // Agent subscriptions
            .route("/subscribe", post(handlers::subscribe))
            .route(
                "/subscribe/{agent_id}",
                delete(handlers::unsubscribe),
            )
            .route("/subscriptions", get(handlers::list_subscriptions))
            // SSE event stream
            .route("/events/stream", get(handlers::events_stream))
            .with_state(state);

        tokio::select! {
            result = axum::serve(listener, app) => {
                result.wrap_err("server error")?;
            }
            _ = token.cancelled() => {
                info!("shutting down");
            }
        }

        Ok(())
    }
}
