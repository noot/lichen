use alloy::primitives::{Address, B256};
use eyre::{Result, WrapErr as _};
use protocol::{SubmitRatingRequest, Task, TaskPhase, TaskStatus};
use uuid::Uuid;

/// The coordinator backend — either the HTTP coordinator binary
/// or an on-chain Ethereum smart contract.
pub enum Backend {
    Http(client::CoordinatorClient),
    Onchain(onchain::OnchainClient),
}

impl Backend {
    pub fn http(base_url: &str) -> Self {
        Self::Http(client::CoordinatorClient::new(base_url))
    }

    pub fn onchain(rpc_url: &str, contract_address: Address, private_key: &str) -> Result<Self> {
        Ok(Self::Onchain(onchain::OnchainClient::new(
            rpc_url,
            contract_address,
            private_key,
        )?))
    }

    pub async fn create_task(&self, prompt: &str, num_raters: usize) -> Result<TaskStatus> {
        match self {
            Self::Http(c) => c.create_task(prompt, num_raters).await,
            Self::Onchain(c) => {
                let prompt_hash = B256::from(alloy::primitives::keccak256(prompt.as_bytes()));
                let num_raters_u8 =
                    u8::try_from(num_raters).wrap_err("num_raters exceeds u8::MAX")?;
                let task_id = c.create_task(prompt_hash, num_raters_u8).await?;
                Ok(TaskStatus {
                    task: Task {
                        id: task_id_to_uuid(task_id),
                        prompt: prompt.to_string(),
                    },
                    phase: TaskPhase::AwaitingWork,
                    num_raters_required: num_raters,
                })
            }
        }
    }

    pub async fn list_tasks(&self) -> Result<Vec<TaskStatus>> {
        match self {
            Self::Http(c) => c.list_tasks().await,
            Self::Onchain(c) => {
                let ids = c.get_active_tasks().await?;
                let mut tasks = Vec::new();
                for id in ids {
                    let (task, ratings) = c.get_task(id).await?;
                    tasks.push(onchain_task_to_status(id, &task, &ratings));
                }
                Ok(tasks)
            }
        }
    }

    pub async fn get_task(&self, task_id: Uuid) -> Result<TaskStatus> {
        match self {
            Self::Http(c) => c.get_task(task_id).await,
            Self::Onchain(c) => {
                let id = uuid_to_task_id(task_id);
                let (task, ratings) = c.get_task(id).await?;
                Ok(onchain_task_to_status(id, &task, &ratings))
            }
        }
    }

    pub async fn submit_result(
        &self,
        task_id: Uuid,
        agent_id: &str,
        output: &str,
    ) -> Result<TaskStatus> {
        match self {
            Self::Http(c) => c.submit_result(task_id, agent_id, output).await,
            Self::Onchain(c) => {
                let id = uuid_to_task_id(task_id);
                let output_hash = B256::from(alloy::primitives::keccak256(output.as_bytes()));
                c.submit_result(id, output_hash).await?;
                self.get_task(task_id).await
            }
        }
    }

    pub async fn submit_rating(
        &self,
        task_id: Uuid,
        agent_id: &str,
        signal: bool,
        prediction: f64,
    ) -> Result<TaskStatus> {
        match self {
            Self::Http(c) => c.submit_rating(task_id, agent_id, signal, prediction).await,
            Self::Onchain(c) => {
                let id = uuid_to_task_id(task_id);
                let pred_fixed = onchain::OnchainClient::prediction_to_fixed(prediction);
                c.submit_rating(id, signal, pred_fixed).await?;
                self.get_task(task_id).await
            }
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(c) => write!(f, "http:{c}"),
            Self::Onchain(c) => write!(f, "{c}"),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Map an on-chain u64 task ID to a UUID (deterministic, zero-padded).
fn task_id_to_uuid(id: u64) -> Uuid {
    let mut bytes = [0u8; 16];
    bytes[8..16].copy_from_slice(&id.to_be_bytes());
    Uuid::from_bytes(bytes)
}

/// Extract the on-chain task ID from a UUID.
fn uuid_to_task_id(uuid: Uuid) -> u64 {
    let bytes = uuid.as_bytes();
    u64::from_be_bytes(bytes[8..16].try_into().unwrap())
}

/// Convert on-chain task + ratings into a protocol TaskStatus.
fn onchain_task_to_status(
    id: u64,
    task: &onchain::LichenCoordinator::Task,
    ratings: &[onchain::LichenCoordinator::Rating],
) -> TaskStatus {
    let phase_val: u8 = task.phase;
    let phase = match phase_val {
        0 => TaskPhase::AwaitingWork,
        1 => {
            let converted_ratings: Vec<SubmitRatingRequest> = ratings
                .iter()
                .map(|r| SubmitRatingRequest {
                    task_id: task_id_to_uuid(id),
                    agent_id: format!("{}", r.rater),
                    signal: r.signal,
                    prediction: onchain::OnchainClient::fixed_to_f64(r.prediction),
                })
                .collect();
            TaskPhase::AwaitingRatings {
                worker_id: format!("{}", task.worker),
                worker_output: format!("{}", task.outputHash),
                ratings: converted_ratings,
            }
        }
        2 => {
            let converted_ratings: Vec<SubmitRatingRequest> = ratings
                .iter()
                .map(|r| SubmitRatingRequest {
                    task_id: task_id_to_uuid(id),
                    agent_id: format!("{}", r.rater),
                    signal: r.signal,
                    prediction: onchain::OnchainClient::fixed_to_f64(r.prediction),
                })
                .collect();
            let num_good = ratings.iter().filter(|r| r.signal).count();
            let approval = if ratings.is_empty() {
                0.0
            } else {
                num_good as f64 / ratings.len() as f64
            };
            TaskPhase::Scored {
                worker_id: format!("{}", task.worker),
                worker_output: format!("{}", task.outputHash),
                ratings: converted_ratings,
                scores: vec![], // scores fetched separately via get_score()
                bts_accepted: task.accepted,
                approval,
                accepted: task.accepted,
            }
        }
        _ => TaskPhase::AwaitingWork,
    };

    TaskStatus {
        task: Task {
            id: task_id_to_uuid(id),
            prompt: format!("{}", task.promptHash),
        },
        phase,
        num_raters_required: task.numRatersRequired as usize,
    }
}
