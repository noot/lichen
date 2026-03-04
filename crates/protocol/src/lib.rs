pub mod scoring;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Agent subscription / open-marketplace types ───────────────────────────────

/// Registration request from an agent that wants to receive task notifications.
///
/// The coordinator will POST [`TaskNotification`] to `callback_url` whenever a
/// new task is available.  The agent replies with accept/decline via the
/// coordinator's REST endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeRequest {
    /// Stable identifier for this agent (e.g. "worker-alpha", Ethereum address, …).
    pub agent_id: String,
    /// HTTP(S) endpoint the coordinator will POST [`TaskNotification`] to.
    pub callback_url: String,
    /// Roles this agent can play.  If empty the coordinator defaults to
    /// `[AgentRole::Worker, AgentRole::Rater]`.
    #[serde(default)]
    pub roles: Vec<AgentRole>,
}

/// Roles an agent can fill in the marketplace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Can submit work for a task.
    Worker,
    /// Can submit ratings for task outputs.
    Rater,
}

/// Confirmation returned after a successful subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeResponse {
    pub agent_id: String,
    pub message: String,
}

/// Payload POSTed by the coordinator to each subscribed agent when a new task
/// becomes available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNotification {
    pub task_id: Uuid,
    pub prompt: String,
    /// On-chain task ID (present when coordinator is in on-chain mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onchain_task_id: Option<u64>,
    pub max_raters: u8,
    pub min_raters: u8,
    /// Unix timestamp of the on-chain deadline (0 when off-chain).
    pub deadline: u64,
    /// Coordinator endpoint the agent should call to accept.
    pub accept_url: String,
    /// Coordinator endpoint the agent should call to decline.
    pub decline_url: String,
}

/// Posted by an agent to accept a task offer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptTaskRequest {
    pub agent_id: String,
    /// Role the agent is accepting as.
    pub role: AgentRole,
}

/// Posted by an agent to decline a task offer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclineTaskRequest {
    pub agent_id: String,
    /// Optional human-readable reason (logged, not acted on).
    #[serde(default)]
    pub reason: String,
}

/// Returned after accept/decline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAcceptResponse {
    pub task_id: Uuid,
    /// `"worker"` if this agent won the worker slot, `"rater"` if queued as
    /// rater, or `"declined"`.
    pub role_granted: String,
    pub message: String,
}

/// Posted to trigger on-chain finalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeTaskRequest {
    /// Agent initiating the finalization (for audit / logging).
    pub agent_id: String,
}

/// Posted to trigger on-chain cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelTaskRequest {
    /// Agent initiating the cancellation (for audit / logging).
    pub agent_id: String,
}

/// A task to be completed by a worker agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub prompt: String,
}

/// Posted by a client to create a new task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub prompt: String,
    /// Expected output for the task (optional, defaults to empty string)
    #[serde(default)]
    pub output: String,
    /// Number of raters required before scoring.
    #[serde(default = "default_num_raters")]
    pub num_raters: usize,
    /// Maximum number of raters allowed (for on-chain mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_raters: Option<u8>,
    /// Minimum number of raters required (for on-chain mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_raters: Option<u8>,
    /// Timeout in seconds (for on-chain mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

fn default_num_raters() -> usize {
    3
}

/// Pushed by the worker agent after completing a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitResultRequest {
    pub task_id: Uuid,
    pub agent_id: String,
    pub output: String,
}

/// Pushed by a rater agent after evaluating a task output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRatingRequest {
    pub task_id: Uuid,
    pub agent_id: String,
    /// true = "good", false = "bad"
    pub signal: bool,
    /// Predicted fraction of raters who will say "good" (0.0 to 1.0)
    pub prediction: f64,
}

/// RBTS score result for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreResult {
    pub agent_id: String,
    pub payment: f64,
}

/// Status of a task in the coordinator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub task: Task,
    pub phase: TaskPhase,
    pub num_raters_required: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    /// Waiting for a worker to submit output.
    AwaitingWork,
    /// Worker submitted; waiting for raters.
    AwaitingRatings {
        worker_id: String,
        worker_output: String,
        ratings: Vec<SubmitRatingRequest>,
    },
    /// All ratings in; scored.
    Scored {
        worker_id: String,
        worker_output: String,
        ratings: Vec<SubmitRatingRequest>,
        scores: Vec<ScoreResult>,
        /// Whether "good" was surprisingly popular — i.e. the actual fraction
        /// of "good" votes met or exceeded the average predicted fraction.
        bts_accepted: bool,
        /// Fraction of raters who voted "good" (0.0 to 1.0).
        approval: f64,
        /// Overall verdict: true if approval >= 0.5 and bts_accepted.
        accepted: bool,
    },
}
