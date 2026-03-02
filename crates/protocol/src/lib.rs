pub mod scoring;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
