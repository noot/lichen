use alloy::{
    network::EthereumWallet,
    primitives::{Address, B256, U256},
    providers::ProviderBuilder,
    signers::local::PrivateKeySigner,
    sol,
};
use eyre::{Result, WrapErr as _};

// Generate type-safe bindings from the contract ABI.
sol!(
    #[sol(rpc)]
    LichenCoordinator,
    "abi/LichenCoordinator.json"
);

// Concrete provider type returned by ProviderBuilder::wallet().connect_http()
type WalletProvider = alloy::providers::fillers::FillProvider<
    alloy::providers::fillers::JoinFill<
        alloy::providers::fillers::JoinFill<
            alloy::providers::Identity,
            alloy::providers::fillers::JoinFill<
                alloy::providers::fillers::GasFiller,
                alloy::providers::fillers::JoinFill<
                    alloy::providers::fillers::BlobGasFiller,
                    alloy::providers::fillers::JoinFill<
                        alloy::providers::fillers::NonceFiller,
                        alloy::providers::fillers::ChainIdFiller,
                    >,
                >,
            >,
        >,
        alloy::providers::fillers::WalletFiller<EthereumWallet>,
    >,
    alloy::providers::RootProvider,
>;

type ContractInstance = LichenCoordinator::LichenCoordinatorInstance<WalletProvider>;

/// Client for interacting with the LichenCoordinator smart contract.
pub struct OnchainClient {
    contract: ContractInstance,
    rpc_url: String,
    contract_address: Address,
}

impl OnchainClient {
    /// Create a new on-chain client.
    ///
    /// - `rpc_url` — HTTP RPC endpoint (e.g. `http://localhost:8545` for anvil).
    /// - `contract_address` — deployed LichenCoordinator address.
    /// - `private_key` — hex-encoded private key for signing transactions.
    pub fn new(rpc_url: &str, contract_address: Address, private_key: &str) -> Result<Self> {
        let signer: PrivateKeySigner = private_key
            .parse()
            .wrap_err("failed to parse private key")?;
        let wallet = EthereumWallet::from(signer);

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(rpc_url.parse().wrap_err("invalid RPC URL")?);

        let contract = LichenCoordinator::new(contract_address, provider);

        Ok(Self {
            contract,
            rpc_url: rpc_url.to_string(),
            contract_address,
        })
    }

    // ── Deposit / Withdraw ───────────────────────────────────────────

    /// Deposit ETH into the contract balance.
    pub async fn deposit(&self, amount_wei: U256) -> Result<()> {
        self.contract
            .deposit()
            .value(amount_wei)
            .send()
            .await
            .wrap_err("deposit tx failed")?
            .watch()
            .await
            .wrap_err("deposit tx not confirmed")?;
        Ok(())
    }

    /// Withdraw ETH from the contract balance.
    pub async fn withdraw(&self, amount_wei: U256) -> Result<()> {
        self.contract
            .withdraw(amount_wei)
            .send()
            .await
            .wrap_err("withdraw tx failed")?
            .watch()
            .await
            .wrap_err("withdraw tx not confirmed")?;
        Ok(())
    }

    /// Get the balance of an agent.
    pub async fn balance_of(&self, agent: Address) -> Result<U256> {
        let bal = self
            .contract
            .balances(agent)
            .call()
            .await
            .wrap_err("balances call failed")?;
        Ok(bal)
    }

    // ── Task Lifecycle ───────────────────────────────────────────────

    /// Create a new task. Returns the on-chain task ID.
    pub async fn create_task(&self, prompt_hash: B256, num_raters: u8) -> Result<u64> {
        let receipt = self
            .contract
            .createTask(prompt_hash, num_raters)
            .send()
            .await
            .wrap_err("createTask tx failed")?
            .get_receipt()
            .await
            .wrap_err("createTask receipt failed")?;

        // Parse TaskCreated event to get the task ID.
        for log in receipt.inner.logs() {
            if let Ok(event) = log.log_decode::<LichenCoordinator::TaskCreated>() {
                let id: u64 = event.inner.taskId.try_into().unwrap_or(0);
                return Ok(id);
            }
        }
        eyre::bail!("TaskCreated event not found in receipt")
    }

    /// Submit worker output for a task.
    pub async fn submit_result(&self, task_id: u64, output_hash: B256) -> Result<()> {
        self.contract
            .submitResult(U256::from(task_id), output_hash)
            .send()
            .await
            .wrap_err("submitResult tx failed")?
            .watch()
            .await
            .wrap_err("submitResult tx not confirmed")?;
        Ok(())
    }

    /// Submit a rating. `prediction` is a 64.64 fixed-point value.
    pub async fn submit_rating(
        &self,
        task_id: u64,
        signal: bool,
        prediction_fixed: i128,
    ) -> Result<()> {
        self.contract
            .submitRating(U256::from(task_id), signal, prediction_fixed)
            .send()
            .await
            .wrap_err("submitRating tx failed")?
            .watch()
            .await
            .wrap_err("submitRating tx not confirmed")?;
        Ok(())
    }

    // ── Views ────────────────────────────────────────────────────────

    /// Get a task and its ratings.
    pub async fn get_task(
        &self,
        task_id: u64,
    ) -> Result<(LichenCoordinator::Task, Vec<LichenCoordinator::Rating>)> {
        let result = self
            .contract
            .getTask(U256::from(task_id))
            .call()
            .await
            .wrap_err("getTask call failed")?;
        Ok((result.task, result.taskRatings))
    }

    /// Get all active (non-scored) task IDs.
    pub async fn get_active_tasks(&self) -> Result<Vec<u64>> {
        let result = self
            .contract
            .getActiveTasks()
            .call()
            .await
            .wrap_err("getActiveTasks call failed")?;
        Ok(result
            .iter()
            .map(|id| (*id).try_into().unwrap_or(0u64))
            .collect())
    }

    /// Get the RBTS score/payout for a rater on a task.
    pub async fn get_score(&self, task_id: u64, rater: Address) -> Result<i128> {
        let result = self
            .contract
            .getScore(U256::from(task_id), rater)
            .call()
            .await
            .wrap_err("getScore call failed")?;
        let val: i128 = result.try_into().unwrap_or(0);
        Ok(val)
    }

    /// Get worker reputation (tasksCompleted, approvals).
    pub async fn get_worker_reputation(&self, worker: Address) -> Result<(u64, u64)> {
        let result = self
            .contract
            .getWorkerReputation(worker)
            .call()
            .await
            .wrap_err("getWorkerReputation call failed")?;
        let completed: u64 = result.tasksCompleted.try_into().unwrap_or(0);
        let approvals: u64 = result.approvals.try_into().unwrap_or(0);
        Ok((completed, approvals))
    }

    /// Check if an address has rated a task.
    pub async fn has_rated(&self, task_id: u64, rater: Address) -> Result<bool> {
        let result = self
            .contract
            .hasRated(U256::from(task_id), rater)
            .call()
            .await
            .wrap_err("hasRated call failed")?;
        Ok(result)
    }

    // ── Helpers ──────────────────────────────────────────────────────

    /// Convert a f64 prediction (0.0 to 1.0) to 64.64 fixed-point.
    pub fn prediction_to_fixed(prediction: f64) -> i128 {
        (prediction * (1i128 << 64) as f64) as i128
    }

    /// Convert a 64.64 fixed-point value to f64.
    pub fn fixed_to_f64(fixed: i128) -> f64 {
        fixed as f64 / (1i128 << 64) as f64
    }
}

impl std::fmt::Display for OnchainClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "onchain:{}@{}", self.rpc_url, self.contract_address)
    }
}
