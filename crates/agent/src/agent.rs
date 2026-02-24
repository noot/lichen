use std::future::IntoFuture as _;

use axum::{routing::get, Router};
use eyre::{Result, WrapErr as _};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::cli::{AgentRole, Args};
use crate::llm::LlmClient;
use crate::polling;

pub struct Agent {
    pub(crate) agent_id: String,
    pub(crate) llm: LlmClient,
    pub(crate) coordinator: client::CoordinatorClient,
    pub(crate) role: AgentRole,
    pub(crate) poll_interval: u64,
    port: u16,
}

impl Agent {
    pub fn new(args: Args) -> Self {
        Self {
            agent_id: args.agent_id,
            llm: LlmClient::new(args.llm_url, args.model, args.api_key, args.provider),
            coordinator: client::CoordinatorClient::new(&args.coordinator_url),
            role: args.role,
            poll_interval: args.poll_interval,
            port: args.port,
        }
    }

    pub async fn run(self, token: CancellationToken) -> Result<()> {
        let Self {
            agent_id,
            llm,
            coordinator,
            role,
            poll_interval,
            port,
        } = self;

        let app = Router::new().route("/health", get(|| async { "ok" }));

        let addr = format!("0.0.0.0:{}", port);
        info!(
            "agent {} ({:?}) listening on {}, polling {}",
            agent_id, role, addr, coordinator
        );
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .wrap_err("failed to bind listener")?;

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(poll_interval));
        let server = axum::serve(listener, app).into_future();
        tokio::pin!(server);

        loop {
            tokio::select! {
                result = &mut server => {
                    result.wrap_err("server error")?;
                    break;
                }
                _ = token.cancelled() => {
                    info!("shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = polling::poll_once(&agent_id, &role, &llm, &coordinator).await {
                        error!("poll error: {e}");
                    }
                }
            }
        }

        Ok(())
    }
}
