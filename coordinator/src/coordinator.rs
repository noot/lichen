use std::{collections::HashMap, sync::Arc};

use axum::{
    routing::{get, post},
    Router,
};
use eyre::{Result, WrapErr as _};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::cli::Args;
use crate::handlers;
use protocol::TaskStatus;

pub struct Coordinator {
    pub(crate) alpha: f64,
    pub(crate) beta: f64,
    pub(crate) tasks: RwLock<HashMap<Uuid, TaskStatus>>,
    port: u16,
}

impl Coordinator {
    pub fn new(args: Args) -> Self {
        Self {
            alpha: args.alpha,
            beta: args.beta,
            tasks: RwLock::new(HashMap::new()),
            port: args.port,
        }
    }

    pub async fn run(self, token: CancellationToken) -> Result<()> {
        let state = Arc::new(self);

        let app = Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/tasks", post(handlers::create_task))
            .route("/tasks", get(handlers::list_tasks))
            .route("/tasks/{task_id}", get(handlers::get_task))
            .route("/tasks/{task_id}/result", post(handlers::submit_result))
            .route("/tasks/{task_id}/rating", post(handlers::submit_rating))
            .with_state(state.clone());

        let addr = format!("0.0.0.0:{}", state.port);
        info!("coordinator listening on {}", addr);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .wrap_err("failed to bind listener")?;

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
