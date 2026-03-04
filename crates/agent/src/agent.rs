use std::future::IntoFuture as _;
use std::sync::Arc;

use axum::{routing::get, Router};
use eyre::{Result, WrapErr as _};
use protocol::AgentRole as ProtocolAgentRole;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::backend::Backend;
use crate::cli::{AgentRole, Args, BackendMode};
use crate::llm::LlmClient;
use crate::subscription::{SubscriptionConfig, SubscriptionState};
use crate::{polling, subscription};

pub struct Agent {
    pub(crate) agent_id: String,
    pub(crate) llm: LlmClient,
    pub(crate) backend: Backend,
    pub(crate) role: AgentRole,
    pub(crate) poll_interval: u64,
    port: u16,
    sub_config: Option<SubscriptionConfig>,
}

impl Agent {
    pub fn new(args: Args) -> Result<Self> {
        let backend = match args.backend {
            BackendMode::Http => Backend::http(&args.coordinator_url),
            BackendMode::Onchain => {
                let contract_address = args.contract_address.as_deref().ok_or_else(|| {
                    eyre::eyre!("--contract-address required for onchain backend")
                })?;
                let private_key = std::env::var("PRIVATE_KEY")
                    .wrap_err("PRIVATE_KEY env var required for onchain backend")?;
                let addr: alloy::primitives::Address = contract_address
                    .parse()
                    .wrap_err("invalid contract address")?;
                Backend::onchain(&args.rpc_url, addr, &private_key)?
            }
        };

        // Build the subscription config if --subscribe is set
        let sub_config = if args.subscribe {
            let callback_url = args.callback_url.clone().unwrap_or_else(|| {
                format!("http://localhost:{}/notify", args.port)
            });
            let roles = vec![match args.role {
                AgentRole::Worker => ProtocolAgentRole::Worker,
                AgentRole::Rater => ProtocolAgentRole::Rater,
            }];
            Some(SubscriptionConfig {
                agent_id: args.agent_id.clone(),
                callback_url,
                roles,
                max_concurrent_tasks: args.max_concurrent_tasks,
                decline_probability: args.decline_probability,
                coordinator_url: args.coordinator_url.clone(),
            })
        } else {
            None
        };

        Ok(Self {
            agent_id: args.agent_id,
            llm: LlmClient::new(args.llm_url, args.model, args.api_key, args.provider),
            backend,
            role: args.role,
            poll_interval: args.poll_interval,
            port: args.port,
            sub_config,
        })
    }

    pub async fn run(self, token: CancellationToken) -> Result<()> {
        let Self {
            agent_id,
            llm,
            backend,
            role,
            poll_interval,
            port,
            sub_config,
        } = self;

        if let Some(cfg) = sub_config {
            run_subscription_mode(agent_id, llm, backend, cfg, token).await
        } else {
            run_polling_mode(agent_id, llm, backend, role, poll_interval, port, token).await
        }
    }
}

// ── Polling mode (original behaviour) ────────────────────────────────────────

async fn run_polling_mode(
    agent_id: String,
    llm: LlmClient,
    backend: Backend,
    role: AgentRole,
    poll_interval: u64,
    port: u16,
    token: CancellationToken,
) -> Result<()> {
    let app = Router::new().route("/health", get(|| async { "ok" }));

    let addr = format!("0.0.0.0:{}", port);
    info!(
        "agent {} ({:?}) listening on {}, polling {}",
        agent_id, role, addr, backend
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
                if let Err(e) = polling::poll_once(&agent_id, &role, &llm, &backend).await {
                    error!("poll error: {e}");
                }
            }
        }
    }

    Ok(())
}

// ── Subscription mode (push-based) ───────────────────────────────────────────

async fn run_subscription_mode(
    agent_id: String,
    llm: LlmClient,
    backend: Backend,
    cfg: SubscriptionConfig,
    token: CancellationToken,
) -> Result<()> {
    info!(
        "agent {} starting in subscription mode (callback: {})",
        agent_id, cfg.callback_url
    );

    // Parse the port from the callback URL for binding (fall back to 0 = OS pick)
    let callback_port: u16 = cfg
        .callback_url
        .rsplit(':')
        .next()
        .and_then(|s| s.split('/').next()) // strip path if present
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let state = Arc::new(SubscriptionState::new(cfg.clone(), llm, backend));

    // Start callback server
    let bound_port =
        subscription::run_subscription_server(callback_port, Arc::clone(&state), token.clone())
            .await?;

    // If callback_url had port 0, update the config with the bound port.
    // (For production use, callback_url should be fully specified.)
    let effective_callback = if callback_port == 0 {
        let base = cfg
            .callback_url
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim_end_matches(':');
        format!("{base}:{bound_port}/notify")
    } else {
        cfg.callback_url.clone()
    };

    // Build the effective config for subscription registration
    let effective_cfg = SubscriptionConfig {
        callback_url: effective_callback,
        ..cfg
    };

    // Register with coordinator
    subscription::subscribe(&effective_cfg)
        .await
        .wrap_err("failed to subscribe with coordinator")?;

    // Wait for shutdown
    token.cancelled().await;
    info!("agent {} shutting down, unsubscribing", agent_id);

    // Graceful unsubscribe
    subscription::unsubscribe(&effective_cfg).await;

    Ok(())
}
