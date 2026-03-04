use std::io::Write as _;
use std::sync::Arc;

use agent::{LlmClient, Message, Provider};
use alloy::primitives::{keccak256, Address, B256, U256};
use alloy::signers::local::PrivateKeySigner;
use eyre::{Result, WrapErr as _};
use futures::stream::StreamExt as _;
use onchain::OnchainClient;
use protocol::scoring::rbts_score;
use protocol::SubmitRatingRequest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

const WORKER_MODEL: &str = "claude-sonnet-4-6";
const STARTING_BALANCE: f64 = 100.0;
const COLLATERAL: f64 = 1.0;
const DEFAULT_NUM_ROUNDS: usize = 100;
const ALPHA: f64 = 1.0;
const BETA: f64 = 1.0;
const HISTORY_WINDOW: usize = 10;

// 1 ETH in wei
const ETH_WEI: u64 = 1_000_000_000_000_000_000;

// Anvil RPC URL (local)
const ANVIL_RPC: &str = "http://127.0.0.1:8545";

/// All 30 deterministic private keys for:
///   anvil --accounts 30 --mnemonic "test test test test test test test test test test test junk"
const ANVIL_KEYS: &[&str] = &[
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
    "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
    "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97",
    "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6",
    "0xf214f2b2cd398c806f84e317254e0f0b801d0643303237d97a22a48e01628897",
    "0x701b615bbdfb9de65240bc28bd21bbc0d996645a3dd57e7b12bc2bdf6f192c82",
    "0xa267530f49f8280200edf313ee7af6b827f2a8bce2897751d06a843f644967b1",
    "0x47c99abed3324a2707c28affff1267e45918ec8c3f20b8aa892e8b065d2942dd",
    "0xc526ee95bf44d8fc405a158bb884d9d1238d99f0612e9f33d006bb0789009aaa",
    "0x8166f546bab6da521a8369cab06c5d2b9e46670292d85c875ee9ec20e84ffb61",
    "0xea6c44ac03bff858b476bba40716402b03e41b8e97e276d1baec7c37d42484a0",
    "0x689af8efa8c651a91ad287602527f3af2fe9f6501a7ac4b061667b5a93e037fd",
    "0xde9be858da4a475276426320d5e9262ecfc3ba460bfac56360bfa6c4c28b4ee0",
    "0xdf57089febbacf7ba0bc227dafbffa9fc08a93fdc68e1e42411a14efcf23656e",
    "0xeaa861a9a01391ed3d587d8a5a84ca56ee277629a8b02c22093a419bf240e65d",
    "0xc511b2aa70776d4ff1d376e8537903dae36896132c90b91d52c1dfbae267cd8b",
    "0x224b7eb7449992aac96d631d9677f7bf5888245eef6d6eeda31e62d2f29a83e4",
    "0x4624e0802698b9769f5bdb260a3777fbd4941ad2901f5966b854f953497eec1b",
    "0x375ad145df13ed97f8ca8e27bb21ebf2a3819e9e0a06509a812db377e533def7",
    "0x18743e59419b01d1d846d97ea070b5a3368a3e7f6f0242cf497e1baac6972427",
    "0xe383b226df7c8282489889170b0f68f66af6459261f4833a781acd0804fafe7a",
    "0xf3a6b71b94f5cd909fb2dbb287da47badaa6d8bcdc45d595e2884835d8749001",
    "0x4e249d317253b9641e477aba8dd5d8f1f7cf5250a5acadd1229693e262720a19",
    "0x233c86e887ac435d7f7dc64979d7758d69320906a0d340d2b6518b0fd20aa998",
];

/// Default rater lineup: (label, model, starting_balance)
/// Labels allow the same model to appear multiple times with different configs.
const RATERS_DEFAULT: &[(&str, &str, f64)] = &[
    ("claude-haiku-4-5", "claude-haiku-4-5", 100.0),
    ("claude-3-5-haiku", "claude-3-5-haiku", 100.0),
    ("claude-sonnet-4-5", "claude-sonnet-4-5", 100.0),
    ("claude-sonnet-4", "claude-sonnet-4", 100.0),
    ("claude-3-7-sonnet", "claude-3-7-sonnet", 100.0),
    ("claude-opus-4-1", "claude-opus-4-1", 100.0),
    ("claude-sonnet-4-6", "claude-sonnet-4-6", 100.0),
    ("gpt-4o", "gpt-4o", 100.0),
    ("gpt-4o-mini", "gpt-4o-mini", 100.0),
    ("gpt-4.1", "gpt-4.1", 100.0),
    ("gpt-4.1-mini", "gpt-4.1-mini", 100.0),
    ("gpt-4.1-nano", "gpt-4.1-nano", 100.0),
    ("gpt-5", "gpt-5", 100.0),
    ("gpt-5-mini", "gpt-5-mini", 100.0),
    ("gpt-5.2", "gpt-5.2", 100.0),
    ("gemini-2.0-flash", "gemini-2.0-flash", 100.0),
    ("gemini-2.5-flash", "gemini-2.5-flash", 100.0),
    ("gemini-2.5-pro", "gemini-2.5-pro", 100.0),
    ("gemini-3-flash", "gemini-3-flash", 100.0),
    ("gemini-3-pro", "gemini-3-pro", 100.0),
    ("gemini-3.1-pro", "gemini-3.1-pro", 100.0),
    ("o3", "o3", 100.0),
    ("o4-mini", "o4-mini", 100.0),
];

/// Bankroll fork experiment: same model at different starting balances
const RATERS_BANKROLL: &[(&str, &str, f64)] = &[
    // Duplicated models at 50/100/200 (confirmed balance-sensitive)
    ("gpt-4.1:poor", "gpt-4.1", 50.0),
    ("gpt-4.1:default", "gpt-4.1", 100.0),
    ("gpt-4.1:rich", "gpt-4.1", 200.0),
    ("gemini-2.0-flash:poor", "gemini-2.0-flash", 50.0),
    ("gemini-2.0-flash:default", "gemini-2.0-flash", 100.0),
    ("gemini-2.0-flash:rich", "gemini-2.0-flash", 200.0),
    ("claude-3-7-sonnet:poor", "claude-3-7-sonnet", 50.0),
    ("claude-3-7-sonnet:default", "claude-3-7-sonnet", 100.0),
    ("claude-3-7-sonnet:rich", "claude-3-7-sonnet", 200.0),
    // Controls at default balance
    ("gpt-5", "gpt-5", 100.0),
    ("claude-sonnet-4-6", "claude-sonnet-4-6", 100.0),
    ("gemini-2.5-pro", "gemini-2.5-pro", 100.0),
    ("gemini-3.1-pro", "gemini-3.1-pro", 100.0),
    ("o4-mini", "o4-mini", 100.0),
    ("gpt-5.2", "gpt-5.2", 100.0),
];

const TASKS: &[&str] = &[
    // easy
    "Write a Rust function to reverse a string.",
    "Write a Rust function to check if a number is prime.",
    "Write a Rust FizzBuzz implementation.",
    "Write a Rust function to find the max element in a slice.",
    "Write a Rust function to count vowels in a string.",
    "Implement a stack data structure in Rust.",
    "Write a Rust function to check if a string is a palindrome.",
    "Write a Rust function to compute fibonacci numbers.",
    "Implement binary search in Rust.",
    "Write a Rust function to flatten a nested list.",
    // medium
    "Implement an LRU cache in Rust.",
    "Write Rust code to serialize and deserialize a binary tree.",
    "Implement a trie data structure in Rust.",
    "Write a Rust function to generate all permutations of a list.",
    "Implement Dijkstra's shortest path algorithm in Rust.",
    "Implement a token bucket rate limiter in Rust.",
    "Implement a bounded blocking queue in Rust.",
    "Write an arithmetic expression evaluator in Rust.",
    "Implement a basic regex matcher in Rust.",
    "Implement the longest common subsequence algorithm in Rust.",
    // hard
    "Implement a lock-free stack in Rust using atomics.",
    "Implement a B-tree in Rust.",
    "Implement the Raft leader election protocol in Rust.",
    "Implement Batcher's odd-even mergesort in Rust.",
    "Implement a persistent (immutable) red-black tree in Rust.",
    "Implement a tri-color garbage collector in Rust.",
    "Write a basic SQL SELECT parser in Rust.",
    "Implement Lamport timestamps for distributed ordering in Rust.",
    "Implement a skip list in Rust.",
    "Solve the N-queens problem in Rust.",
    // impossible
    "Write a single Rust expression that sorts a vector, reverses it, and computes median and standard deviation.",
    "Write a quantum computing simulator in Rust in under 50 lines.",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoundRecord {
    round: usize,
    task: String,
    worker_output: String,
    ratings: Vec<RaterRecord>,
    consensus: ConsensusStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    onchain_task_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RaterRecord {
    model: String,
    signal: bool,
    prediction: f64,
    rbts_score: f64,
    payout: f64,
    balance_after: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConsensusStats {
    num_good: usize,
    num_rated: usize,
    approval_pct: f64,
    avg_prediction: f64,
    bts_accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RaterResponse {
    signal: bool,
    prediction: f64,
}

struct RaterState {
    label: String,
    model: String,
    balance: f64,
    eliminated: bool,
    history: Vec<RoundHistoryEntry>,
}

#[derive(Clone)]
struct RoundHistoryEntry {
    round: usize,
    task: String,
    own_signal: bool,
    own_prediction: f64,
    others: Vec<(String, bool, f64)>,
    consensus: ConsensusStats,
    payout: f64,
    balance_after: f64,
}

fn build_rater_prompt(
    state: &RaterState,
    task: &str,
    worker_output: &str,
    worker_reputation: Option<(u64, u64)>,
) -> String {
    let mut prompt = format!(
        "You are a rater in a decentralized quality assurance protocol with real economic stakes.\n\n\
         === YOUR ECONOMIC STATE ===\n\
         Current balance: {:.2} points\n\
         Collateral this round: {:.2} points\n",
        state.balance, COLLATERAL
    );

    if let Some((completed, approvals)) = worker_reputation {
        if completed > 0 {
            let approval_rate = approvals as f64 / completed as f64 * 100.0;
            prompt.push_str(&format!(
                "\n=== WORKER REPUTATION ===\n\
                 Tasks completed: {}\n\
                 Approved: {} ({:.0}%)\n",
                completed, approvals, approval_rate
            ));
        } else {
            prompt.push_str("\n=== WORKER REPUTATION ===\nNew worker (no history)\n");
        }
    }

    if !state.history.is_empty() {
        let start = state.history.len().saturating_sub(HISTORY_WINDOW);
        prompt.push_str("\n=== YOUR RECENT HISTORY ===\n");
        for entry in &state.history[start..] {
            let vote = if entry.own_signal { "GOOD" } else { "BAD" };
            prompt.push_str(&format!(
                "\nRound {}:\n  Task: {}\n  Your vote: {}, Your prediction: {:.2}\n  \
                 Consensus: {:.0}% GOOD ({}/{}), Avg prediction: {:.2}, BTS accepted: {}\n  \
                 Your payout: {:+.4} → balance: {:.2}\n  Other raters: ",
                entry.round,
                entry.task,
                vote,
                entry.own_prediction,
                entry.consensus.approval_pct,
                entry.consensus.num_good,
                entry.consensus.num_rated,
                entry.consensus.avg_prediction,
                entry.consensus.bts_accepted,
                entry.payout,
                entry.balance_after,
            ));
            let others: Vec<String> = entry
                .others
                .iter()
                .map(|(m, s, p)| {
                    format!("{}: {} (pred={:.2})", m, if *s { "GOOD" } else { "BAD" }, p)
                })
                .collect();
            prompt.push_str(&others.join(", "));
            prompt.push('\n');
        }
    }

    prompt.push_str(&format!(
        "\nIf your balance hits 0, you're eliminated. Honest, accurate ratings are REWARDED.\n\
         Dishonest or lazy ratings are PUNISHED via Bayesian Truth Serum scoring.\n\
         Study your history carefully.\n\n\
         === TASK ===\n{task}\n\n\
         === WORKER OUTPUT ===\n{worker_output}\n\n\
         Rate this as GOOD or BAD based on correctness, completeness, and quality.\n\
         Then predict what fraction of other raters will say GOOD (0.0 to 1.0).\n\n\
         Respond ONLY as JSON: {{\"signal\": true, \"prediction\": 0.75}}"
    ));
    prompt
}

#[allow(clippy::arithmetic_side_effects)]
fn parse_rater_response(raw: &str) -> Option<RaterResponse> {
    let text = raw.trim();
    if let Ok(r) = serde_json::from_str::<RaterResponse>(text) {
        return Some(r);
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text[start..].rfind('}') {
            if let Ok(r) = serde_json::from_str::<RaterResponse>(&text[start..=start + end]) {
                return Some(r);
            }
        }
    }
    None
}

mod payouts;

/// Deploy LichenCoordinator on the already-running anvil node.
/// Returns deployed contract address.
async fn deploy_contract(rpc_url: &str, deployer_key: &str) -> Result<Address> {
    use alloy::network::{EthereumWallet, TransactionBuilder as _};
    use alloy::providers::{Provider as _, ProviderBuilder};
    use alloy::rpc::types::TransactionRequest;
    use alloy::sol_types::SolValue;

    let artifact_json =
        include_str!("../../../contracts/out/LichenCoordinator.sol/LichenCoordinator.json");
    let artifact: serde_json::Value =
        serde_json::from_str(artifact_json).wrap_err("failed to parse contract artifact")?;
    let hex_bytecode = artifact["bytecode"]["object"]
        .as_str()
        .ok_or_else(|| eyre::eyre!("bytecode.object missing from artifact"))?;
    let hex_bytecode = hex_bytecode.trim_start_matches("0x");
    let bytecode =
        alloy::primitives::hex::decode(hex_bytecode).wrap_err("failed to decode bytecode hex")?;

    // alpha=1<<64, beta=1<<64, collateral=1 ETH
    let alpha_fixed: i128 = 1i128 << 64;
    let beta_fixed: i128 = 1i128 << 64;
    let collateral_wei = U256::from(ETH_WEI);

    let constructor_args = (alpha_fixed, beta_fixed, collateral_wei).abi_encode();
    let mut deploy_data = bytecode;
    deploy_data.extend_from_slice(&constructor_args);

    let signer: PrivateKeySigner = deployer_key
        .parse()
        .wrap_err("failed to parse deployer key")?;
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(rpc_url.parse().wrap_err("invalid rpc url")?);

    let tx = TransactionRequest::default().with_deploy_code(deploy_data);
    let pending = provider
        .send_transaction(tx)
        .await
        .wrap_err("send deploy tx failed")?;
    let receipt = pending
        .get_receipt()
        .await
        .wrap_err("deploy receipt failed")?;
    let addr = receipt
        .contract_address
        .ok_or_else(|| eyre::eyre!("no contract address in deploy receipt"))?;
    Ok(addr)
}

/// Key index 0 = worker, keys 1..=25 = raters
struct OnchainSetup {
    _anvil: std::process::Child,
    worker_client: OnchainClient,
    worker_address: Address,
    rater_clients: Vec<(OnchainClient, Address)>, // (client, address) for each rater
}

#[allow(clippy::arithmetic_side_effects)]
async fn setup_onchain() -> Result<OnchainSetup> {
    use std::time::Duration;

    // Spawn anvil
    println!("[onchain] Spawning anvil...");
    let anvil = std::process::Command::new("anvil")
        .args([
            "--accounts",
            "30",
            "--balance",
            "1000",
            "--mnemonic",
            "test test test test test test test test test test test junk",
            "--port",
            "8545",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .wrap_err("failed to spawn anvil")?;

    // Wait for anvil to be ready — keep polling until we get a valid JSON-RPC response
    let http_client = reqwest::Client::new();
    let mut ready = false;
    for attempt in 0..60 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "net_version",
            "params": [],
            "id": 1
        });
        match http_client.post(ANVIL_RPC).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                // Actually parse the response to ensure anvil is fully up
                if resp.text().await.unwrap_or_default().contains("result") {
                    ready = true;
                    println!("[onchain] Anvil ready after {}ms", (attempt + 1) * 500);
                    break;
                }
            }
            _ => {}
        }
    }
    if !ready {
        eyre::bail!("Anvil did not become ready in time");
    }
    // Extra safety margin
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("[onchain] Anvil ready at {ANVIL_RPC}");

    // Deploy contract using key[0]
    let contract_address = deploy_contract(ANVIL_RPC, ANVIL_KEYS[0]).await?;
    println!("[onchain] Contract deployed at {contract_address}");

    // Create worker client (key[0])
    let worker_signer: PrivateKeySigner = ANVIL_KEYS[0].parse().wrap_err("bad worker key")?;
    let worker_address = worker_signer.address();
    let worker_client = OnchainClient::new(ANVIL_RPC, contract_address, ANVIL_KEYS[0])?;

    // Create rater clients (keys 1..=25)
    let mut rater_clients = Vec::new();
    for key in &ANVIL_KEYS[1..=25] {
        let signer: PrivateKeySigner = key.parse().wrap_err("bad rater key")?;
        let addr = signer.address();
        let client = OnchainClient::new(ANVIL_RPC, contract_address, key)?;
        rater_clients.push((client, addr));
    }

    // Each rater deposits 100 ETH
    println!("[onchain] Depositing 100 ETH for each rater...");
    for (client, _addr) in &rater_clients {
        #[allow(clippy::arithmetic_side_effects)]
        let amount = U256::from(100u128 * ETH_WEI as u128);
        client
            .deposit(amount)
            .await
            .wrap_err("rater deposit failed")?;
    }
    println!("[onchain] All raters deposited. Setup complete.");

    Ok(OnchainSetup {
        _anvil: anvil,
        worker_client,
        worker_address,
        rater_clients,
    })
}

enum RoundOutcome {
    Completed,
    Skipped,
    StopSimulation,
}

#[allow(clippy::too_many_arguments, clippy::arithmetic_side_effects)]
async fn run_round(
    round: usize,
    num_rounds: usize,
    raters: &mut [RaterState],
    total_approvals: &mut usize,
    total_rounds_completed: &mut usize,
    jsonl_file: &mut std::fs::File,
    llm_worker_client: &LlmClient,
    provider_url: &str,
    provider_key: &str,
    onchain_setup: Option<&OnchainSetup>,
    stationary_task: Option<&str>,
    max_concurrent: usize,
) -> Result<RoundOutcome> {
    let task_idx = (round - 1) % TASKS.len();
    let task = stationary_task.unwrap_or(TASKS[task_idx]);

    let active: Vec<usize> = raters
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.eliminated)
        .map(|(i, _)| i)
        .collect();

    if active.len() < 2 {
        println!("Round {round}: fewer than 2 active raters, stopping.");
        return Ok(RoundOutcome::StopSimulation);
    }

    println!(
        "--- Round {round}/{num_rounds} ({} active raters) ---",
        active.len()
    );
    println!("Task: {}", &task[..task.len().min(80)]);

    let worker_output = match llm_worker_client
        .chat(&[Message {
            role: "user".to_string(),
            content: format!("{task}\n\nProvide a complete, working Rust implementation."),
        }])
        .await
    {
        Ok(output) => output,
        Err(e) => {
            println!("  Worker failed: {e}, skipping round");
            return Ok(RoundOutcome::Skipped);
        }
    };
    println!("  Worker output: {} chars", worker_output.len());

    let onchain_task_id: Option<u64> = if let Some(setup) = onchain_setup {
        let prompt_hash = B256::from(keccak256(task.as_bytes()));
        let output_hash = B256::from(keccak256(worker_output.as_bytes()));
        let num_raters: u8 = active
            .len()
            .try_into()
            .wrap_err("active rater count exceeds u8::MAX")?;

        // Use num_raters as both max and min
        let max_raters = num_raters;
        let min_raters = num_raters;
        // Default timeout of 24 hours
        let timeout_seconds = alloy::primitives::U256::from(24 * 60 * 60);

        match setup
            .worker_client
            .create_task(
                prompt_hash,
                output_hash,
                max_raters,
                min_raters,
                timeout_seconds,
            )
            .await
        {
            Ok(tid) => {
                println!("  [onchain] Task created with output, id={tid}");
                Some(tid)
            }
            Err(e) => {
                println!("  [onchain] create_task failed: {e}, skipping round");
                return Ok(RoundOutcome::Skipped);
            }
        }
    } else {
        None
    };

    let worker_rep: Option<(u64, u64)> = if let Some(setup) = onchain_setup {
        setup
            .worker_client
            .get_worker_reputation(setup.worker_address)
            .await
            .ok()
    } else {
        Some((*total_rounds_completed as u64, *total_approvals as u64))
    };

    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut futures = futures::stream::FuturesUnordered::new();
    for &idx in &active {
        let label = raters[idx].label.clone();
        let model = raters[idx].model.clone();
        let prompt = build_rater_prompt(&raters[idx], task, &worker_output, worker_rep);
        let url = provider_url.to_owned();
        let key = provider_key.to_owned();
        let client = LlmClient::new(url, model.clone(), Some(key), Provider::Openai);
        let sem = semaphore.clone();
        futures.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let result = client
                .chat(&[Message {
                    role: "user".to_string(),
                    content: prompt,
                }])
                .await;
            (idx, label, model, result)
        }));
    }

    // (idx, label, model, response) — label is unique identity, model is the LLM to call
    let mut responses: Vec<(usize, String, String, RaterResponse)> = Vec::new();
    while let Some(result) = futures.next().await {
        match result {
            Ok((idx, label, _model, Ok(raw))) => match parse_rater_response(&raw) {
                Some(resp) => {
                    let resp = RaterResponse {
                        signal: resp.signal,
                        prediction: resp.prediction.clamp(0.01, 0.99),
                    };
                    responses.push((idx, label, raters[idx].model.clone(), resp));
                }
                None => {
                    println!("  {label}: failed to parse response, defaulting GOOD/0.5");
                    responses.push((
                        idx,
                        label,
                        raters[idx].model.clone(),
                        RaterResponse {
                            signal: true,
                            prediction: 0.5,
                        },
                    ));
                }
            },
            Ok((idx, label, _model, Err(e))) => {
                println!("  {label}: API error ({e}), defaulting GOOD/0.5");
                responses.push((
                    idx,
                    label,
                    raters[idx].model.clone(),
                    RaterResponse {
                        signal: true,
                        prediction: 0.5,
                    },
                ));
            }
            Err(e) => {
                println!("  rater task panicked: {e}");
            }
        }
    }

    if responses.len() < 2 {
        println!("  Fewer than 2 responses, skipping round");
        return Ok(RoundOutcome::Skipped);
    }

    // on-chain: submit ratings via smart contract
    let mut onchain_balances: Option<HashMap<Address, f64>> = None;

    if let (Some(setup), Some(tid)) = (onchain_setup, onchain_task_id) {
        for (idx, _label, _model, resp) in &responses {
            let rater_idx = *idx;
            if rater_idx >= setup.rater_clients.len() {
                println!("  [onchain] WARNING: rater_idx={rater_idx} out of bounds, skipping");
                continue;
            }
            let (ref client, _) = setup.rater_clients[rater_idx];
            let pred_fixed = OnchainClient::prediction_to_fixed(resp.prediction);
            match client.submit_rating(tid, resp.signal, pred_fixed).await {
                Ok(()) => {}
                Err(e) => {
                    println!("  [onchain] submit_rating failed for rater {rater_idx}: {e}");
                }
            }
        }

        let mut balances = HashMap::new();
        for (idx, _label, _model, _resp) in &responses {
            let rater_idx = *idx;
            if rater_idx >= setup.rater_clients.len() {
                continue;
            }
            let (ref client, addr) = setup.rater_clients[rater_idx];
            match client.balance_of(addr).await {
                Ok(wei) => {
                    let points = wei.to::<u128>() as f64 / ETH_WEI as f64;
                    if rater_idx == 0 {
                        println!("  [onchain] rater0 raw_wei={wei} points={points:.6}");
                    }
                    balances.insert(addr, points);
                }
                Err(e) => {
                    println!("  [onchain] balance_of failed for rater {idx}: {e}");
                }
            }
        }
        onchain_balances = Some(balances);
    }

    let task_uuid = Uuid::new_v4();
    let submit_ratings: Vec<SubmitRatingRequest> = responses
        .iter()
        .map(|(_, label, _, resp)| SubmitRatingRequest {
            task_id: task_uuid,
            agent_id: label.clone(),
            signal: resp.signal,
            prediction: resp.prediction,
        })
        .collect();

    let scores = rbts_score(&submit_ratings, ALPHA, BETA);
    let payouts = payouts::zero_sum_payouts(&scores, active.len());

    let num_good = responses.iter().filter(|(_, _, _, r)| r.signal).count();
    let num_rated = responses.len();
    let approval_pct = (num_good as f64 / num_rated as f64) * 100.0;
    let avg_prediction = responses
        .iter()
        .map(|(_, _, _, r)| r.prediction)
        .sum::<f64>()
        / num_rated as f64;
    let actual_good_frac = num_good as f64 / num_rated as f64;
    let bts_accepted = actual_good_frac >= avg_prediction;
    if bts_accepted && actual_good_frac >= 0.5 {
        *total_approvals += 1;
    }
    *total_rounds_completed += 1;

    let consensus = ConsensusStats {
        num_good,
        num_rated,
        approval_pct,
        avg_prediction,
        bts_accepted,
    };

    println!(
        "  Consensus: {:.0}% GOOD ({}/{}), avg pred: {:.2}, BTS accepted: {}",
        approval_pct, num_good, num_rated, avg_prediction, bts_accepted
    );

    let all_votes: Vec<(String, bool, f64)> = responses
        .iter()
        .map(|(_, label, _, r)| (label.clone(), r.signal, r.prediction))
        .collect();

    let mut rater_records = Vec::new();
    for (idx, label, _model, resp) in &responses {
        let payout = payouts.get(label.as_str()).copied().unwrap_or(0.0);
        let rbts = scores
            .iter()
            .find(|s| s.agent_id == *label)
            .map(|s| s.payment)
            .unwrap_or(0.0);

        // Off-chain balance update: deduct collateral, then add payout (mirrors on-chain logic)
        raters[*idx].balance = raters[*idx].balance - COLLATERAL + payout;

        let rater_addr = onchain_setup
            .and_then(|s| s.rater_clients.get(*idx))
            .map(|(_, addr)| *addr);
        let (balance_after, eliminated) = if let Some(ref bals) = onchain_balances {
            let onchain_bal = rater_addr.and_then(|a| bals.get(&a)).copied();
            if let Some(onchain_points) = onchain_bal {
                // Verify off-chain matches on-chain
                let offchain_bal = raters[*idx].balance;
                if (offchain_bal - onchain_points).abs() > 0.01 {
                    println!(
                        "  ⚠️  BALANCE MISMATCH {}: offchain={:.4} onchain={:.4} (diff={:.4})",
                        label,
                        offchain_bal,
                        onchain_points,
                        offchain_bal - onchain_points
                    );
                }
                (onchain_points, onchain_points < 1.0)
            } else {
                let bal = raters[*idx].balance;
                if bal <= 0.0 {
                    (0.0, true)
                } else {
                    (bal, false)
                }
            }
        } else {
            let bal = raters[*idx].balance;
            if bal <= 0.0 {
                (0.0, true)
            } else {
                (bal, false)
            }
        };

        raters[*idx].balance = balance_after;
        if eliminated && !raters[*idx].eliminated {
            raters[*idx].eliminated = true;
            println!("  ☠️  {} ELIMINATED (balance: {:.2})", label, balance_after);
        }

        let others: Vec<(String, bool, f64)> = all_votes
            .iter()
            .filter(|(l, _, _)| l != label)
            .cloned()
            .collect();

        raters[*idx].history.push(RoundHistoryEntry {
            round,
            task: task.to_string(),
            own_signal: resp.signal,
            own_prediction: resp.prediction,
            others,
            consensus: consensus.clone(),
            payout,
            balance_after,
        });

        rater_records.push(RaterRecord {
            model: label.clone(),
            signal: resp.signal,
            prediction: resp.prediction,
            rbts_score: rbts,
            payout,
            balance_after,
        });

        let vote = if resp.signal { "GOOD" } else { "BAD" };
        println!(
            "  {} {} pred={:.2} payout={:+.4} bal={:.2}",
            label, vote, resp.prediction, payout, raters[*idx].balance
        );
    }

    let record = RoundRecord {
        round,
        task: task.to_string(),
        worker_output: worker_output.clone(),
        ratings: rater_records,
        consensus,
        onchain_task_id,
    };
    writeln!(jsonl_file, "{}", serde_json::to_string(&record)?)?;
    jsonl_file.flush()?;
    println!();

    Ok(RoundOutcome::Completed)
}

#[tokio::main]
#[allow(clippy::arithmetic_side_effects)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    // Parse CLI flags
    let args: Vec<String> = std::env::args().collect();
    let use_onchain = args.iter().any(|a| a == "--onchain");
    let use_bankroll = args.iter().any(|a| a == "--bankroll");
    let use_stationary = args.iter().any(|a| a == "--stationary");
    let num_rounds = args
        .iter()
        .position(|a| a == "--rounds")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_NUM_ROUNDS);
    let custom_rater_count = args
        .iter()
        .position(|a| a == "--raters")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok());
    let max_concurrent = args
        .iter()
        .position(|a| a == "--max-concurrent")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(6);

    let provider_url = std::env::var("LLM_API_URL")
        .wrap_err("LLM_API_URL not set — add it to .env or environment")?;
    let provider_key = std::env::var("LLM_API_KEY")
        .wrap_err("LLM_API_KEY not set — add it to .env or environment")?;

    let llm_worker_client = LlmClient::new(
        provider_url.clone(),
        WORKER_MODEL.to_string(),
        Some(provider_key.clone()),
        Provider::Openai,
    );

    let rater_config = if use_bankroll {
        RATERS_BANKROLL
    } else {
        RATERS_DEFAULT
    };

    let mut raters: Vec<RaterState> = if let Some(n) = custom_rater_count {
        // Generate N raters by cycling through the model pool
        let models: Vec<(&str, &str)> = RATERS_DEFAULT.iter().map(|(l, m, _)| (*l, *m)).collect();
        (0..n)
            .map(|i| {
                let (base_label, model) = models[i % models.len()];
                let label = if n > models.len() {
                    format!("{}:{}", base_label, i / models.len())
                } else {
                    base_label.to_string()
                };
                RaterState {
                    label,
                    model: model.to_string(),
                    balance: STARTING_BALANCE,
                    eliminated: false,
                    history: Vec::new(),
                }
            })
            .collect()
    } else {
        rater_config
            .iter()
            .map(|(label, model, balance)| RaterState {
                label: label.to_string(),
                model: model.to_string(),
                balance: *balance,
                eliminated: false,
                history: Vec::new(),
            })
            .collect()
    };

    let (jsonl_path, summary_path) = if custom_rater_count.is_some() {
        (
            "lichen-economy-rounds-marketplace.jsonl",
            "lichen-economy-summary-marketplace.md",
        )
    } else if use_stationary {
        (
            "lichen-economy-rounds-stationary.jsonl",
            "lichen-economy-summary-stationary.md",
        )
    } else if use_bankroll {
        (
            "lichen-economy-rounds-bankroll.jsonl",
            "lichen-economy-summary-bankroll.md",
        )
    } else if use_onchain {
        (
            "lichen-economy-rounds-onchain.jsonl",
            "lichen-economy-summary-onchain.md",
        )
    } else {
        ("lichen-economy-rounds.jsonl", "lichen-economy-summary.md")
    };

    let mut jsonl_file = std::fs::File::create(jsonl_path)?;
    let mut total_approvals: usize = 0;
    let mut total_rounds_completed: usize = 0;

    let mode_tag = if use_stationary {
        " (STATIONARY REGIME)"
    } else if use_bankroll {
        " (BANKROLL FORK)"
    } else if use_onchain {
        " (ON-CHAIN)"
    } else {
        ""
    };
    println!("=== LICHEN ECONOMY SIMULATOR{mode_tag} ===");
    println!(
        "Raters: {}, Rounds: {num_rounds}, Max concurrent: {max_concurrent}",
        raters.len()
    );
    println!();

    let onchain_setup = if use_onchain {
        Some(setup_onchain().await?)
    } else {
        None
    };

    let total_gas_used: u64 = 0;

    let stationary_task: Option<&str> = if use_stationary {
        Some("Write a Rust function to reverse a string.")
    } else {
        None
    };

    for round in 1..=num_rounds {
        match run_round(
            round,
            num_rounds,
            &mut raters,
            &mut total_approvals,
            &mut total_rounds_completed,
            &mut jsonl_file,
            &llm_worker_client,
            &provider_url,
            &provider_key,
            onchain_setup.as_ref(),
            stationary_task,
            max_concurrent,
        )
        .await?
        {
            RoundOutcome::Completed | RoundOutcome::Skipped => {}
            RoundOutcome::StopSimulation => break,
        }
    }

    // write summary
    let mut summary = String::new();
    summary.push_str("# Lichen Economy Simulation Summary\n\n");
    summary.push_str(&format!("- **Rounds:** {num_rounds}\n"));
    summary.push_str(&format!("- **Starting raters:** {}\n", raters.len()));
    if use_stationary {
        summary.push_str("- **Mode:** Stationary regime (same task every round)\n");
        summary.push_str("- **Task:** Write a Rust function to reverse a string.\n");
    } else if use_bankroll {
        summary.push_str("- **Mode:** Bankroll fork experiment (varied starting balances)\n");
    }
    summary.push_str(&format!("- **Starting balance:** {}\n", STARTING_BALANCE));
    summary.push_str(&format!("- **Collateral per round:** {}\n", COLLATERAL));
    if use_onchain {
        summary.push_str("- **Mode:** On-chain via LichenCoordinator smart contract\n");
        summary.push_str(&format!("- **Total gas used:** {} units\n", total_gas_used));
    }
    summary.push('\n');

    // sort by balance descending
    let mut standings: Vec<(String, f64, bool)> = raters
        .iter()
        .map(|r| (r.label.clone(), r.balance, r.eliminated))
        .collect();
    standings.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    summary.push_str("## Final Standings\n\n");
    summary.push_str("| Rank | Model | Balance | Status |\n");
    summary.push_str("|------|-------|---------|--------|\n");
    for (i, (model, balance, eliminated)) in standings.iter().enumerate() {
        let status = if *eliminated {
            "❌ Eliminated"
        } else {
            "✅ Active"
        };
        summary.push_str(&format!(
            "| {} | {} | {:.2} | {} |\n",
            i + 1,
            model,
            balance,
            status
        ));
    }

    let eliminated_count = raters.iter().filter(|r| r.eliminated).count();
    let active_count = raters.iter().filter(|r| !r.eliminated).count();
    summary.push_str("\n## Statistics\n\n");
    summary.push_str(&format!(
        "- **Rounds completed:** {total_rounds_completed}\n"
    ));
    summary.push_str(&format!(
        "- **Worker approvals:** {total_approvals}/{total_rounds_completed} ({:.0}%)\n",
        total_approvals as f64 / total_rounds_completed.max(1) as f64 * 100.0
    ));
    summary.push_str(&format!("- **Survived:** {active_count}\n"));
    summary.push_str(&format!("- **Eliminated:** {eliminated_count}\n"));

    if let Some(best) = standings.first() {
        summary.push_str(&format!(
            "- **Top performer:** {} ({:.2})\n",
            best.0, best.1
        ));
    }
    if let Some(worst) = standings.last() {
        summary.push_str(&format!(
            "- **Worst performer:** {} ({:.2})\n",
            worst.0, worst.1
        ));
    }

    std::fs::write(summary_path, &summary)?;
    println!("=== SIMULATION COMPLETE ===");
    println!(
        "Worker approvals: {total_approvals}/{total_rounds_completed} ({:.0}%)",
        total_approvals as f64 / total_rounds_completed.max(1) as f64 * 100.0
    );
    if use_onchain {
        println!("Total gas used: {total_gas_used} units");
    }
    println!("Results written to {jsonl_path} and {summary_path}");

    for (i, (model, balance, _)) in standings.iter().enumerate() {
        println!("  #{}: {} — {:.2}", i + 1, model, balance);
    }

    Ok(())
}
