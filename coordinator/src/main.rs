use clap::Parser as _;
use eyre::{Result, WrapErr as _};
use tokio_util::sync::CancellationToken;
use tracing::info;

use coordinator::cli::Args;
use coordinator::Coordinator;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let token = CancellationToken::new();

    let run_token = token.clone();
    let coordinator = Coordinator::new(Args::parse());
    let handle = tokio::spawn(async move { coordinator.run(run_token).await });

    tokio::select! {
        result = handle => {
            result
                .wrap_err("coordinator task panicked")?
                .wrap_err("coordinator failed")?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received shutdown signal");
            token.cancel();
        }
    }

    Ok(())
}
