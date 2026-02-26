use clap::Parser;
use eyre::{Result, WrapErr as _};
use tokio_util::sync::CancellationToken;
use tracing::info;

use agent::{Agent, Args};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let token = CancellationToken::new();

    let run_token = token.clone();
    let agent = Agent::new(Args::parse())?;
    let handle = tokio::spawn(async move { agent.run(run_token).await });

    tokio::select! {
        result = handle => {
            result
                .wrap_err("agent task panicked")?
                .wrap_err("agent failed")?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received shutdown signal");
            token.cancel();
        }
    }

    Ok(())
}
