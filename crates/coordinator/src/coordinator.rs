use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use eyre::{Result, WrapErr as _};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::backend::{InMemoryBackend, OnchainBackend, TaskBackend};
use crate::cli::Args;
use crate::handlers;

#[derive(Clone, Debug)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum TaskEvent {
    TaskCreated {
        task_id: uuid::Uuid,
        prompt: String,
    },
    TaskRated {
        task_id: uuid::Uuid,
        agent_id: String,
    },
    TaskScored {
        task_id: uuid::Uuid,
        accepted: bool,
    },
}

pub(crate) struct AppState {
    pub(crate) alpha: f64,
    pub(crate) beta: f64,
    pub(crate) backend: Arc<dyn TaskBackend>,
    pub(crate) event_tx: broadcast::Sender<TaskEvent>,
}

pub struct Coordinator {
    alpha: f64,
    beta: f64,
    backend: Arc<dyn TaskBackend>,
}

impl Coordinator {
    pub fn new(args: &Args) -> Result<Self> {
        let backend: Arc<dyn TaskBackend> = if args.onchain {
            // Parse contract address
            let contract_address = args
                .contract_address
                .as_ref()
                .ok_or_else(|| eyre::eyre!("contract_address required when --onchain is set"))?
                .parse()
                .wrap_err("invalid contract address")?;

            // Create on-chain client
            let client = onchain::OnchainClient::new(
                args.rpc_url
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("rpc_url required when --onchain is set"))?,
                contract_address,
                args.private_key
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("private_key required when --onchain is set"))?,
            )
            .wrap_err("failed to create on-chain client")?;

            Arc::new(OnchainBackend::new(client))
        } else {
            Arc::new(InMemoryBackend::new())
        };

        Ok(Self {
            alpha: args.alpha,
            beta: args.beta,
            backend,
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
        } = self;

        info!(
            "coordinator listening on {}",
            listener.local_addr().wrap_err("failed to get local addr")?
        );

        let (event_tx, _rx) = broadcast::channel(100);

        let state = Arc::new(AppState {
            alpha,
            beta,
            backend,
            event_tx: event_tx.clone(),
        });

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/tasks", post(handlers::create_task))
            .route("/tasks", get(handlers::list_tasks))
            .route("/tasks/{task_id}", get(handlers::get_task))
            .route("/tasks/{task_id}/result", post(handlers::submit_result))
            .route("/tasks/{task_id}/rating", post(handlers::submit_rating))
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
