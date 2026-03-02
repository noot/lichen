use std::collections::HashMap;

use alloy::primitives::{B256, U256};
use eyre::{Result, WrapErr as _};
use protocol::{ScoreResult, SubmitRatingRequest, Task, TaskPhase, TaskStatus};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use onchain::{LichenCoordinator, OnchainClient};

/// Backend for task storage and management.
#[async_trait::async_trait]
pub(crate) trait TaskBackend: Send + Sync {
    /// Create a new task.
    async fn create_task(
        &self,
        prompt: String,
        output: String,
        num_raters: usize,
        max_raters: Option<u8>,
        min_raters: Option<u8>,
        timeout_seconds: Option<u64>,
    ) -> Result<TaskStatus>;

    /// Get a task by ID.
    async fn get_task(&self, task_id: Uuid) -> Result<Option<TaskStatus>>;

    /// List all tasks.
    async fn list_tasks(&self) -> Result<Vec<TaskStatus>>;

    /// Submit a result for a task.
    async fn submit_result(
        &self,
        task_id: Uuid,
        agent_id: String,
        output: String,
    ) -> Result<TaskStatus>;

    /// Submit a rating for a task.
    async fn submit_rating(
        &self,
        task_id: Uuid,
        agent_id: String,
        signal: bool,
        prediction: f64,
        alpha: f64,
        beta: f64,
    ) -> Result<TaskStatus>;
}

/// In-memory backend for task storage.
pub(crate) struct InMemoryBackend {
    tasks: RwLock<HashMap<Uuid, TaskStatus>>,
}

impl InMemoryBackend {
    pub(crate) fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl TaskBackend for InMemoryBackend {
    async fn create_task(
        &self,
        prompt: String,
        _output: String,
        num_raters: usize,
        _max_raters: Option<u8>,
        _min_raters: Option<u8>,
        _timeout_seconds: Option<u64>,
    ) -> Result<TaskStatus> {
        let task_id = Uuid::new_v4();
        let status = TaskStatus {
            task: Task {
                id: task_id,
                prompt,
            },
            phase: TaskPhase::AwaitingWork,
            num_raters_required: num_raters,
        };

        self.tasks.write().await.insert(task_id, status.clone());
        info!("created task {task_id}");

        Ok(status)
    }

    async fn get_task(&self, task_id: Uuid) -> Result<Option<TaskStatus>> {
        Ok(self.tasks.read().await.get(&task_id).cloned())
    }

    async fn list_tasks(&self) -> Result<Vec<TaskStatus>> {
        Ok(self.tasks.read().await.values().cloned().collect())
    }

    async fn submit_result(
        &self,
        task_id: Uuid,
        agent_id: String,
        output: String,
    ) -> Result<TaskStatus> {
        let mut tasks = self.tasks.write().await;
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| eyre::eyre!("task not found"))?;

        if !matches!(task.phase, TaskPhase::AwaitingWork) {
            return Err(eyre::eyre!("task already has a result"));
        }

        task.phase = TaskPhase::AwaitingRatings {
            worker_id: agent_id.clone(),
            worker_output: output,
            ratings: vec![],
        };

        info!("task {task_id}: result submitted by {agent_id}");
        Ok(task.clone())
    }

    async fn submit_rating(
        &self,
        task_id: Uuid,
        agent_id: String,
        signal: bool,
        prediction: f64,
        alpha: f64,
        beta: f64,
    ) -> Result<TaskStatus> {
        use protocol::scoring;

        let mut tasks = self.tasks.write().await;
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| eyre::eyre!("task not found"))?;

        // validate phase and check for duplicate raters
        match &task.phase {
            TaskPhase::AwaitingWork => {
                return Err(eyre::eyre!("task has no result yet"));
            }
            TaskPhase::Scored { .. } => {
                return Err(eyre::eyre!("task already scored"));
            }
            TaskPhase::AwaitingRatings { ratings, .. } => {
                if ratings.iter().any(|r| r.agent_id == agent_id) {
                    return Err(eyre::eyre!("agent already rated this task"));
                }
            }
        }

        let rating = SubmitRatingRequest {
            task_id,
            agent_id: agent_id.clone(),
            signal,
            prediction,
        };

        // push rating and check if we should score
        let should_score = {
            let TaskPhase::AwaitingRatings { ratings, .. } = &mut task.phase else {
                unreachable!()
            };
            ratings.push(rating);
            info!(
                "task {task_id}: rating from {} ({}/{})",
                agent_id,
                ratings.len(),
                task.num_raters_required
            );
            ratings.len() >= task.num_raters_required
        };

        if should_score {
            let TaskPhase::AwaitingRatings {
                worker_id,
                worker_output,
                ratings,
            } = std::mem::replace(&mut task.phase, TaskPhase::AwaitingWork)
            else {
                unreachable!()
            };

            let scores = scoring::rbts_score(&ratings, alpha, beta);
            let actual_good =
                ratings.iter().filter(|r| r.signal).count() as f64 / ratings.len() as f64;
            let predicted_good =
                ratings.iter().map(|r| r.prediction).sum::<f64>() / ratings.len() as f64;
            let bts_accepted = actual_good >= predicted_good;
            let approval = actual_good;
            let accepted = approval >= 0.5 && bts_accepted;

            info!("task {task_id}: scored (accepted={accepted}, approval={approval:.2}, bts_accepted={bts_accepted}) — {scores:?}");
            task.phase = TaskPhase::Scored {
                worker_id,
                worker_output,
                ratings,
                scores,
                bts_accepted,
                approval,
                accepted,
            };
        }

        Ok(task.clone())
    }
}

/// On-chain backend for task storage.
pub(crate) struct OnchainBackend {
    client: OnchainClient,
    /// Maps Uuid to on-chain task ID
    task_mapping: RwLock<HashMap<Uuid, u64>>,
    /// Reverse mapping: on-chain ID to Uuid
    reverse_mapping: RwLock<HashMap<u64, Uuid>>,
}

impl OnchainBackend {
    pub(crate) fn new(client: OnchainClient) -> Self {
        Self {
            client,
            task_mapping: RwLock::new(HashMap::new()),
            reverse_mapping: RwLock::new(HashMap::new()),
        }
    }

    fn hash_string(s: &str) -> B256 {
        let mut hasher = Sha256::new();
        hasher.update(s.as_bytes());
        B256::from_slice(&hasher.finalize())
    }

    async fn convert_onchain_task(
        &self,
        onchain_id: u64,
        task: &LichenCoordinator::Task,
        ratings: &[LichenCoordinator::Rating],
    ) -> Result<TaskStatus> {
        // Get or create UUID for this on-chain task
        let uuid = {
            let reverse = self.reverse_mapping.read().await;
            if let Some(&uuid) = reverse.get(&onchain_id) {
                uuid
            } else {
                drop(reverse);
                let uuid = Uuid::new_v4();
                self.task_mapping.write().await.insert(uuid, onchain_id);
                self.reverse_mapping.write().await.insert(onchain_id, uuid);
                uuid
            }
        };

        // Determine phase based on task state
        // phase: 0 = AwaitingRatings, 1 = Scored, 2 = Cancelled
        let phase = if task.phase == 1 {
            // Task is scored - reconstruct the scored phase
            let mut task_ratings = Vec::new();
            let mut scores = Vec::new();

            for rating in ratings {
                task_ratings.push(SubmitRatingRequest {
                    task_id: uuid,
                    agent_id: format!("{:?}", rating.rater),
                    signal: rating.signal,
                    prediction: OnchainClient::fixed_to_f64(rating.prediction),
                });

                // Score would need to be retrieved separately using get_score
                scores.push(ScoreResult {
                    agent_id: format!("{:?}", rating.rater),
                    payment: 0.0, // TODO: retrieve using get_score
                });
            }

            let actual_good =
                task_ratings.iter().filter(|r| r.signal).count() as f64 / task_ratings.len() as f64;
            let predicted_good =
                task_ratings.iter().map(|r| r.prediction).sum::<f64>() / task_ratings.len() as f64;
            let bts_accepted = actual_good >= predicted_good;
            let approval = actual_good;
            let accepted = approval >= 0.5 && bts_accepted;

            TaskPhase::Scored {
                worker_id: format!("{:?}", task.worker),
                worker_output: "".to_string(), // We don't store output on-chain
                ratings: task_ratings,
                scores,
                bts_accepted,
                approval,
                accepted,
            }
        } else if !ratings.is_empty() {
            // Has ratings but not scored
            let mut task_ratings = Vec::new();
            for rating in ratings {
                task_ratings.push(SubmitRatingRequest {
                    task_id: uuid,
                    agent_id: format!("{:?}", rating.rater),
                    signal: rating.signal,
                    prediction: OnchainClient::fixed_to_f64(rating.prediction),
                });
            }

            TaskPhase::AwaitingRatings {
                worker_id: format!("{:?}", task.worker),
                worker_output: "".to_string(), // We don't store output on-chain
                ratings: task_ratings,
            }
        } else {
            TaskPhase::AwaitingWork
        };

        Ok(TaskStatus {
            task: Task {
                id: uuid,
                prompt: "".to_string(), // We don't store prompt on-chain, only hash
            },
            phase,
            num_raters_required: task.minRaters as usize,
        })
    }
}

#[async_trait::async_trait]
impl TaskBackend for OnchainBackend {
    async fn create_task(
        &self,
        prompt: String,
        output: String,
        num_raters: usize,
        max_raters: Option<u8>,
        min_raters: Option<u8>,
        timeout_seconds: Option<u64>,
    ) -> Result<TaskStatus> {
        let prompt_hash = Self::hash_string(&prompt);
        let output_hash = Self::hash_string(&output);

        let max_raters = max_raters.unwrap_or(10);
        #[allow(clippy::cast_possible_truncation)]
        let min_raters = min_raters.unwrap_or(num_raters.min(255) as u8);
        let timeout_seconds = timeout_seconds.unwrap_or(3600);

        let onchain_id = self
            .client
            .create_task(
                prompt_hash,
                output_hash,
                max_raters,
                min_raters,
                U256::from(timeout_seconds),
            )
            .await
            .wrap_err("failed to create on-chain task")?;

        let task_id = Uuid::new_v4();
        self.task_mapping.write().await.insert(task_id, onchain_id);
        self.reverse_mapping
            .write()
            .await
            .insert(onchain_id, task_id);

        info!("created on-chain task {task_id} (on-chain ID: {onchain_id})");

        Ok(TaskStatus {
            task: Task {
                id: task_id,
                prompt,
            },
            phase: TaskPhase::AwaitingWork,
            num_raters_required: num_raters,
        })
    }

    async fn get_task(&self, task_id: Uuid) -> Result<Option<TaskStatus>> {
        let onchain_id = {
            let mapping = self.task_mapping.read().await;
            match mapping.get(&task_id) {
                Some(&id) => id,
                None => return Ok(None),
            }
        };

        let (task, ratings) = self
            .client
            .get_task(onchain_id)
            .await
            .wrap_err("failed to get on-chain task")?;

        let status = self
            .convert_onchain_task(onchain_id, &task, &ratings)
            .await?;
        Ok(Some(status))
    }

    async fn list_tasks(&self) -> Result<Vec<TaskStatus>> {
        let active_ids = self
            .client
            .get_active_tasks()
            .await
            .wrap_err("failed to get active tasks")?;

        let mut tasks = Vec::new();
        for onchain_id in active_ids {
            let (task, ratings) = self
                .client
                .get_task(onchain_id)
                .await
                .wrap_err("failed to get task details")?;

            let status = self
                .convert_onchain_task(onchain_id, &task, &ratings)
                .await?;
            tasks.push(status);
        }

        Ok(tasks)
    }

    async fn submit_result(
        &self,
        _task_id: Uuid,
        _agent_id: String,
        _output: String,
    ) -> Result<TaskStatus> {
        // On-chain tasks don't have a separate "submit result" phase
        // The worker is set when the task is created
        Err(eyre::eyre!("on-chain tasks don't support submit_result"))
    }

    async fn submit_rating(
        &self,
        task_id: Uuid,
        agent_id: String,
        signal: bool,
        prediction: f64,
        _alpha: f64,
        _beta: f64,
    ) -> Result<TaskStatus> {
        let onchain_id = {
            let mapping = self.task_mapping.read().await;
            *mapping
                .get(&task_id)
                .ok_or_else(|| eyre::eyre!("task not found"))?
        };

        let prediction_fixed = OnchainClient::prediction_to_fixed(prediction);

        self.client
            .submit_rating(onchain_id, signal, prediction_fixed)
            .await
            .wrap_err("failed to submit on-chain rating")?;

        info!(
            "submitted on-chain rating for task {} (on-chain ID: {}) by {}",
            task_id, onchain_id, agent_id
        );

        // Fetch updated task status
        let (task, ratings) = self
            .client
            .get_task(onchain_id)
            .await
            .wrap_err("failed to get updated task")?;

        let status = self
            .convert_onchain_task(onchain_id, &task, &ratings)
            .await?;

        // Check if we should finalize
        if ratings.len() >= task.maxRaters as usize {
            self.client
                .finalize_task(onchain_id)
                .await
                .wrap_err("failed to finalize task")?;
            info!(
                "finalized on-chain task {} (reached max raters)",
                onchain_id
            );
        }

        Ok(status)
    }
}
