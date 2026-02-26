use alloy::{
    network::EthereumWallet,
    node_bindings::{Anvil, AnvilInstance},
    primitives::{keccak256, Address, B256, U256},
    providers::{Provider as _, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use onchain::OnchainClient;

/// Anvil default private keys (deterministic).
const ANVIL_KEY_0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ANVIL_KEY_1: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const ANVIL_KEY_2: &str = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const ANVIL_KEY_3: &str = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";

/// alpha=1, beta=1 as 64.64 fixed-point (1 << 64).
const ALPHA_FIXED: i128 = 1i128 << 64;
const COLLATERAL: u64 = 1_000_000_000_000_000_000; // 1 ETH in wei

/// Contract bytecode (compiled with forge, extracted from artifacts).
const CONTRACT_BYTECODE: &str = include_str!("../../../contracts/out/LichenCoordinator.sol/LichenCoordinator.json");

/// Deploy the contract on anvil via a raw provider, return the address.
async fn deploy_contract(anvil: &AnvilInstance) -> Address {
    let signer: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(anvil.endpoint().parse().unwrap());

    // Extract bytecode from forge artifact JSON
    let artifact: serde_json::Value = serde_json::from_str(CONTRACT_BYTECODE).unwrap();
    let hex_bytecode = artifact["bytecode"]["object"].as_str().unwrap();
    let bytecode = alloy::primitives::hex::decode(hex_bytecode).unwrap();

    // Encode constructor args: (int128 alpha, int128 beta, uint256 collateral)
    use alloy::sol_types::SolValue;
    let constructor_args = (ALPHA_FIXED, ALPHA_FIXED, U256::from(COLLATERAL)).abi_encode();

    let mut deploy_data = bytecode;
    deploy_data.extend_from_slice(&constructor_args);

    use alloy::network::TransactionBuilder as _;
    let tx = alloy::rpc::types::TransactionRequest::default()
        .with_deploy_code(deploy_data);

    let pending = provider.send_transaction(tx).await.expect("send deploy tx");
    let receipt = pending.get_receipt().await.expect("deploy receipt");
    receipt.contract_address.expect("no contract address in receipt")
}

/// Create a client for a given anvil key index.
fn client_for(anvil: &AnvilInstance, key: &str, addr: Address) -> OnchainClient {
    OnchainClient::new(&anvil.endpoint(), addr, key).unwrap()
}

// ── Helper conversion tests ──────────────────────────────────────────

#[test]
fn test_prediction_to_fixed_and_back() {
    let cases = [0.0, 0.25, 0.5, 0.75, 0.9, 0.95, 1.0];
    for &p in &cases {
        let fixed = OnchainClient::prediction_to_fixed(p);
        let back = OnchainClient::fixed_to_f64(fixed);
        assert!(
            (back - p).abs() < 1e-10,
            "roundtrip failed for {p}: got {back}"
        );
    }
}

#[test]
fn test_prediction_boundary_values() {
    // 0.0 should be 0 in fixed-point
    assert_eq!(OnchainClient::prediction_to_fixed(0.0), 0);
    // 1.0 should be 1 << 64
    assert_eq!(OnchainClient::prediction_to_fixed(1.0), 1i128 << 64);
}

// ── Contract integration tests (require anvil) ──────────────────────

#[tokio::test]
async fn test_deposit_and_balance() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;
    let client = client_for(&anvil, ANVIL_KEY_0, addr);

    let signer: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let agent_addr = signer.address();

    // Balance should start at 0
    let bal = client.balance_of(agent_addr).await.unwrap();
    assert_eq!(bal, U256::ZERO);

    // Deposit 5 ETH
    client.deposit(U256::from(5 * COLLATERAL)).await.unwrap();
    let bal = client.balance_of(agent_addr).await.unwrap();
    assert_eq!(bal, U256::from(5 * COLLATERAL));

    // Deposit more
    client.deposit(U256::from(3 * COLLATERAL)).await.unwrap();
    let bal = client.balance_of(agent_addr).await.unwrap();
    assert_eq!(bal, U256::from(8 * COLLATERAL));
}

#[tokio::test]
async fn test_withdraw() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;
    let client = client_for(&anvil, ANVIL_KEY_0, addr);

    let signer: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let agent_addr = signer.address();

    client.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client.withdraw(U256::from(4 * COLLATERAL)).await.unwrap();

    let bal = client.balance_of(agent_addr).await.unwrap();
    assert_eq!(bal, U256::from(6 * COLLATERAL));
}

#[tokio::test]
async fn test_create_task_and_get() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;
    let client = client_for(&anvil, ANVIL_KEY_0, addr);

    let prompt_hash = B256::from(keccak256(b"write fizzbuzz"));
    let task_id = client.create_task(prompt_hash, 3).await.unwrap();
    assert_eq!(task_id, 0); // first task

    let (task, ratings) = client.get_task(task_id).await.unwrap();
    assert_eq!(task.promptHash, prompt_hash);
    assert_eq!(task.numRatersRequired, 3);
    assert_eq!(task.phase, 0); // AwaitingWork
    assert!(ratings.is_empty());
}

#[tokio::test]
async fn test_active_tasks() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;
    let client = client_for(&anvil, ANVIL_KEY_0, addr);

    assert!(client.get_active_tasks().await.unwrap().is_empty());

    let id0 = client
        .create_task(B256::from(keccak256(b"task0")), 2)
        .await
        .unwrap();
    let id1 = client
        .create_task(B256::from(keccak256(b"task1")), 2)
        .await
        .unwrap();

    let active = client.get_active_tasks().await.unwrap();
    assert_eq!(active.len(), 2);
    assert!(active.contains(&id0));
    assert!(active.contains(&id1));
}

#[tokio::test]
async fn test_submit_result() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;
    let client = client_for(&anvil, ANVIL_KEY_0, addr);

    let task_id = client
        .create_task(B256::from(keccak256(b"prompt")), 2)
        .await
        .unwrap();

    let output_hash = B256::from(keccak256(b"fn fizzbuzz() { ... }"));
    client.submit_result(task_id, output_hash).await.unwrap();

    let (task, _) = client.get_task(task_id).await.unwrap();
    assert_eq!(task.phase, 1); // AwaitingRatings
    assert_eq!(task.outputHash, output_hash);
}

#[tokio::test]
async fn test_submit_rating_and_has_rated() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;

    // Use key 0 to create task + submit result
    let client0 = client_for(&anvil, ANVIL_KEY_0, addr);
    client0.deposit(U256::from(10 * COLLATERAL)).await.unwrap();

    let task_id = client0
        .create_task(B256::from(keccak256(b"prompt")), 3)
        .await
        .unwrap();
    client0
        .submit_result(task_id, B256::from(keccak256(b"output")))
        .await
        .unwrap();

    // Rate with key 0
    let signer0: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let pred = OnchainClient::prediction_to_fixed(0.8);
    client0.submit_rating(task_id, true, pred).await.unwrap();

    assert!(client0.has_rated(task_id, signer0.address()).await.unwrap());

    // Key 1 hasn't rated yet
    let signer1: PrivateKeySigner = ANVIL_KEY_1.parse().unwrap();
    assert!(!client0.has_rated(task_id, signer1.address()).await.unwrap());

    // Check ratings
    let (_, ratings) = client0.get_task(task_id).await.unwrap();
    assert_eq!(ratings.len(), 1);
    assert!(ratings[0].signal);
}

#[tokio::test]
async fn test_full_lifecycle_auto_scores() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;

    let client0 = client_for(&anvil, ANVIL_KEY_0, addr);
    let client1 = client_for(&anvil, ANVIL_KEY_1, addr);
    let client2 = client_for(&anvil, ANVIL_KEY_2, addr);

    // Deposit collateral for all raters
    client0.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client1.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client2.deposit(U256::from(10 * COLLATERAL)).await.unwrap();

    let signer0: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let signer1: PrivateKeySigner = ANVIL_KEY_1.parse().unwrap();
    let signer2: PrivateKeySigner = ANVIL_KEY_2.parse().unwrap();

    // Create task and submit work
    let task_id = client0
        .create_task(B256::from(keccak256(b"implement quicksort")), 3)
        .await
        .unwrap();
    client0
        .submit_result(task_id, B256::from(keccak256(b"fn quicksort() { ... }")))
        .await
        .unwrap();

    // All 3 rate GOOD with prediction 0.9
    let pred = OnchainClient::prediction_to_fixed(0.9);
    client0.submit_rating(task_id, true, pred).await.unwrap();
    client1.submit_rating(task_id, true, pred).await.unwrap();
    client2.submit_rating(task_id, true, pred).await.unwrap();

    // Task should be scored now
    let (task, _) = client0.get_task(task_id).await.unwrap();
    assert_eq!(task.phase, 2); // Scored
    assert!(task.accepted);

    // Active tasks should be empty
    assert!(client0.get_active_tasks().await.unwrap().is_empty());

    // All scored equally — each gets back their 1 ETH collateral
    let score0 = client0.get_score(task_id, signer0.address()).await.unwrap();
    let score1 = client0.get_score(task_id, signer1.address()).await.unwrap();
    let score2 = client0.get_score(task_id, signer2.address()).await.unwrap();
    assert_eq!(score0, score1);
    assert_eq!(score1, score2);
    assert_eq!(score0 as u64, COLLATERAL);

    // Balances should be restored to 10 ETH
    let bal0 = client0.balance_of(signer0.address()).await.unwrap();
    assert_eq!(bal0, U256::from(10 * COLLATERAL));
}

#[tokio::test]
async fn test_surprisingly_popular_rewards() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;

    let client0 = client_for(&anvil, ANVIL_KEY_0, addr);
    let client1 = client_for(&anvil, ANVIL_KEY_1, addr);
    let client2 = client_for(&anvil, ANVIL_KEY_2, addr);
    let client3 = client_for(&anvil, ANVIL_KEY_3, addr);

    client0.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client1.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client2.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client3.deposit(U256::from(10 * COLLATERAL)).await.unwrap();

    let signer0: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let signer3: PrivateKeySigner = ANVIL_KEY_3.parse().unwrap();

    let task_id = client0
        .create_task(B256::from(keccak256(b"task")), 4)
        .await
        .unwrap();
    client0
        .submit_result(task_id, B256::from(keccak256(b"output")))
        .await
        .unwrap();

    // 3 vote GOOD (pred 0.5), 1 votes BAD (pred 0.5)
    // GOOD is surprisingly popular
    let pred = OnchainClient::prediction_to_fixed(0.5);
    client0.submit_rating(task_id, true, pred).await.unwrap();
    client1.submit_rating(task_id, true, pred).await.unwrap();
    client2.submit_rating(task_id, true, pred).await.unwrap();
    client3.submit_rating(task_id, false, pred).await.unwrap();

    let (task, _) = client0.get_task(task_id).await.unwrap();
    assert!(task.accepted);

    // GOOD voter should get more than BAD voter
    let score_good = client0.get_score(task_id, signer0.address()).await.unwrap();
    let score_bad = client0.get_score(task_id, signer3.address()).await.unwrap();
    assert!(
        score_good > score_bad,
        "good voter ({score_good}) should beat bad voter ({score_bad})"
    );
}

#[tokio::test]
async fn test_better_predictor_scores_higher() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;

    let client0 = client_for(&anvil, ANVIL_KEY_0, addr);
    let client1 = client_for(&anvil, ANVIL_KEY_1, addr);

    client0.deposit(U256::from(10 * COLLATERAL)).await.unwrap();
    client1.deposit(U256::from(10 * COLLATERAL)).await.unwrap();

    let signer0: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let signer1: PrivateKeySigner = ANVIL_KEY_1.parse().unwrap();

    let task_id = client0
        .create_task(B256::from(keccak256(b"task")), 2)
        .await
        .unwrap();
    client0
        .submit_result(task_id, B256::from(keccak256(b"output")))
        .await
        .unwrap();

    // Both vote GOOD; client0 predicts 0.95 (accurate), client1 predicts 0.5 (bad)
    let pred_good = OnchainClient::prediction_to_fixed(0.95);
    let pred_bad = OnchainClient::prediction_to_fixed(0.50);
    client0.submit_rating(task_id, true, pred_good).await.unwrap();
    client1.submit_rating(task_id, true, pred_bad).await.unwrap();

    let score0 = client0.get_score(task_id, signer0.address()).await.unwrap();
    let score1 = client0.get_score(task_id, signer1.address()).await.unwrap();
    assert!(
        score0 > score1,
        "better predictor ({score0}) should beat worse predictor ({score1})"
    );
}

#[tokio::test]
async fn test_balances_approximately_zero_sum() {
    let anvil = Anvil::new().spawn();
    let addr = deploy_contract(&anvil).await;

    let client0 = client_for(&anvil, ANVIL_KEY_0, addr);
    let client1 = client_for(&anvil, ANVIL_KEY_1, addr);
    let client2 = client_for(&anvil, ANVIL_KEY_2, addr);
    let client3 = client_for(&anvil, ANVIL_KEY_3, addr);

    let deposit_amt = U256::from(10 * COLLATERAL);
    client0.deposit(deposit_amt).await.unwrap();
    client1.deposit(deposit_amt).await.unwrap();
    client2.deposit(deposit_amt).await.unwrap();
    client3.deposit(deposit_amt).await.unwrap();

    let signer0: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let signer1: PrivateKeySigner = ANVIL_KEY_1.parse().unwrap();
    let signer2: PrivateKeySigner = ANVIL_KEY_2.parse().unwrap();
    let signer3: PrivateKeySigner = ANVIL_KEY_3.parse().unwrap();

    let total_before = U256::from(COLLATERAL) * U256::from(40);

    // Create + work + rate with mixed votes
    let task_id = client0
        .create_task(B256::from(keccak256(b"task")), 4)
        .await
        .unwrap();
    client0
        .submit_result(task_id, B256::from(keccak256(b"output")))
        .await
        .unwrap();

    client0.submit_rating(task_id, true, OnchainClient::prediction_to_fixed(0.9)).await.unwrap();
    client1.submit_rating(task_id, true, OnchainClient::prediction_to_fixed(0.9)).await.unwrap();
    client2.submit_rating(task_id, true, OnchainClient::prediction_to_fixed(0.5)).await.unwrap();
    client3.submit_rating(task_id, false, OnchainClient::prediction_to_fixed(0.2)).await.unwrap();

    let bal0 = client0.balance_of(signer0.address()).await.unwrap();
    let bal1 = client0.balance_of(signer1.address()).await.unwrap();
    let bal2 = client0.balance_of(signer2.address()).await.unwrap();
    let bal3 = client0.balance_of(signer3.address()).await.unwrap();
    let total_after = bal0 + bal1 + bal2 + bal3;

    let diff = if total_before > total_after {
        total_before - total_after
    } else {
        total_after - total_before
    };

    // Allow tiny rounding (< 0.001 ETH)
    assert!(
        diff < U256::from(COLLATERAL / 1000),
        "not zero-sum: before={total_before}, after={total_after}, diff={diff}"
    );
}
