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

    /// Coordinator URL to push results/ratings to
    #[arg(long, default_value = "http://localhost:3000")]
    pub coordinator_url: String,

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
