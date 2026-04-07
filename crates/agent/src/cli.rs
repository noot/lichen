use clap::Parser;

use crate::llm::Provider;

#[derive(Parser)]
pub struct Args {
    #[arg(long, default_value = "3001")]
    pub port: u16,

    #[arg(long, default_value = "agent-1")]
    pub agent_id: String,

    #[arg(long, default_value = "llama3.2")]
    pub model: String,

    #[arg(long, default_value = "http://localhost:11434/v1")]
    pub llm_url: String,

    #[arg(long)]
    pub api_key: Option<String>,

    #[arg(long, value_enum, default_value = "openai")]
    pub provider: Provider,

    /// Coordinator URL to push results/ratings to (HTTP backend)
    #[arg(long, default_value = "http://localhost:3000")]
    pub coordinator_url: String,

    /// Backend mode: "http" or "onchain"
    #[arg(long, default_value = "http")]
    pub backend: BackendMode,

    /// Contract address (required for onchain backend)
    #[arg(long)]
    pub contract_address: Option<String>,

    /// RPC URL for onchain backend
    #[arg(long, default_value = "http://localhost:8545")]
    pub rpc_url: String,

    /// Run mode: "worker" or "rater"
    #[arg(long, default_value = "worker")]
    pub role: AgentRole,

    /// Poll interval in seconds for checking new tasks
    #[arg(long, default_value = "5")]
    pub poll_interval: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentRole {
    Worker,
    Rater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BackendMode {
    Http,
    Onchain,
}
