use clap::Parser;

#[derive(Parser)]
pub struct Args {
    #[arg(long, default_value = "3000")]
    pub port: u16,

    /// Alpha weight for information score in RBTS
    #[arg(long, default_value = "1.0")]
    pub alpha: f64,

    /// Beta weight for prediction score in RBTS
    #[arg(long, default_value = "1.0")]
    pub beta: f64,

    /// Enable on-chain backend for task storage
    #[arg(long)]
    pub onchain: bool,

    /// RPC URL for the blockchain (required if --onchain is set)
    #[arg(long, required_if_eq("onchain", "true"))]
    pub rpc_url: Option<String>,

    /// Contract address for LichenCoordinator (required if --onchain is set)
    #[arg(long, required_if_eq("onchain", "true"))]
    pub contract_address: Option<String>,

    /// Private key for signing transactions (required if --onchain is set)
    #[arg(long, required_if_eq("onchain", "true"))]
    pub private_key: Option<String>,
}
