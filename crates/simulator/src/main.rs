use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;

use agent::{LlmClient, Message, Provider};
use coordinator::scoring::rbts_score;
use eyre::{Result, WrapErr as _};
use protocol::SubmitRatingRequest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
const WORKER_MODEL: &str = "gpt-4.1";
const STARTING_BALANCE: f64 = 100.0;
const COLLATERAL: f64 = 1.0;
const NUM_ROUNDS: usize = 100;
const ALPHA: f64 = 1.0;
const BETA: f64 = 1.0;
const HISTORY_WINDOW: usize = 10;

const MODELS: &[&str] = &[
    "claude-haiku-4-5",
    "claude-3-5-haiku",
    "claude-sonnet-4-5",
    "claude-sonnet-4",
    "claude-3-7-sonnet",
    "claude-opus-4-1",
    "claude-sonnet-4-6",
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-4.1-nano",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5.2",
    "gemini-2.0-flash",
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-3-flash",
    "gemini-3-pro",
    "gemini-3.1-pro",
    "o3",
    "o3-mini",
    "o4-mini",
];

const TASKS: &[&str] = &[
    // Easy
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
    // Medium
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
    // Hard
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
    // Impossible
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
    others: Vec<(String, bool, f64)>, // (model, signal, prediction)
    consensus: ConsensusStats,
    payout: f64,
    balance_after: f64,
}

fn build_rater_prompt(state: &RaterState, task: &str, worker_output: &str) -> String {
    let mut prompt = format!(
        "You are a rater in a decentralized quality assurance protocol with real economic stakes.\n\n\
         === YOUR ECONOMIC STATE ===\n\
         Current balance: {:.2} points\n\
         Collateral this round: {:.2} points\n",
        state.balance, COLLATERAL
    );

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
         Study your history carefully. Learn from rounds where you lost money.\n\n\
         === TASK ===\n{task}\n\n\
         === WORKER OUTPUT ===\n{worker_output}\n\n\
         Rate this as GOOD or BAD based on correctness, completeness, and quality.\n\
         Then predict what fraction of other raters will say GOOD (0.0 to 1.0).\n\n\
         Respond ONLY as JSON: {{\"signal\": true, \"prediction\": 0.75}}"
    ));
    prompt
}

fn parse_rater_response(raw: &str) -> Option<RaterResponse> {
    // Try to find JSON in the response
    let text = raw.trim();
    // Try direct parse first
    if let Ok(r) = serde_json::from_str::<RaterResponse>(text) {
        return Some(r);
    }
    // Try to extract JSON object from the text
    if let Some(start) = text.find('{') {
        if let Some(end) = text[start..].rfind('}') {
            if let Ok(r) = serde_json::from_str::<RaterResponse>(&text[start..=start + end]) {
                return Some(r);
            }
        }
    }
    None
}

fn zero_sum_payouts(scores: &[protocol::ScoreResult], active_count: usize) -> HashMap<String, f64> {
    let pool = active_count as f64 * COLLATERAL;
    let n = scores.len() as f64;
    let mean = scores.iter().map(|s| s.payment).sum::<f64>() / n;
    let centered: Vec<f64> = scores.iter().map(|s| s.payment - mean).collect();
    let total_abs: f64 = centered.iter().map(|x| x.abs()).sum();

    let mut payouts = HashMap::new();
    if total_abs < 1e-12 {
        // Everyone scored the same — no transfers
        for s in scores {
            payouts.insert(s.agent_id.clone(), 0.0);
        }
    } else {
        for (i, s) in scores.iter().enumerate() {
            let payout = centered[i] / total_abs * pool;
            payouts.insert(s.agent_id.clone(), payout);
        }
    }
    payouts
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    dotenvy::dotenv().ok();
    let provider_url = std::env::var("LLM_API_URL")
        .wrap_err("LLM_API_URL not set — add it to .env or environment")?;
    let provider_key = std::env::var("LLM_API_KEY")
        .wrap_err("LLM_API_KEY not set — add it to .env or environment")?;

    let worker_client = LlmClient::new(
        provider_url.clone(),
        WORKER_MODEL.to_string(),
        Some(provider_key.clone()),
        Provider::Openai,
    );

    let mut raters: Vec<RaterState> = MODELS
        .iter()
        .map(|m| RaterState {
            model: m.to_string(),
            balance: STARTING_BALANCE,
            eliminated: false,
            history: Vec::new(),
        })
        .collect();

    let jsonl_path = "lichen-economy-rounds.jsonl";
    let mut jsonl_file = std::fs::File::create(jsonl_path)?;
    let mut total_approvals: usize = 0;
    let mut total_rounds_completed: usize = 0;

    // Limit concurrent API calls to avoid 429s
    let semaphore = Arc::new(tokio::sync::Semaphore::new(6));

    println!("=== LICHEN ECONOMY SIMULATOR ===");
    println!(
        "Raters: {}, Rounds: {}, Starting balance: {}, Max concurrent: 6",
        MODELS.len(),
        NUM_ROUNDS,
        STARTING_BALANCE
    );
    println!();

    for round in 1..=NUM_ROUNDS {
        let task_idx = (round - 1) % TASKS.len();
        let task = TASKS[task_idx];

        let active: Vec<usize> = raters
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.eliminated)
            .map(|(i, _)| i)
            .collect();

        if active.len() < 2 {
            println!("Round {round}: fewer than 2 active raters, stopping.");
            break;
        }

        println!(
            "--- Round {round}/{NUM_ROUNDS} ({} active raters) ---",
            active.len()
        );
        println!("Task: {}", &task[..task.len().min(80)]);

        // Worker generates code
        let worker_output = match worker_client
            .chat(&[Message {
                role: "user".to_string(),
                content: format!("{task}\n\nProvide a complete, working Rust implementation."),
            }])
            .await
        {
            Ok(output) => output,
            Err(e) => {
                println!("  Worker failed: {e}, skipping round");
                continue;
            }
        };
        println!("  Worker output: {} chars", worker_output.len());

        // All raters rate concurrently
        let mut handles = Vec::new();
        for &idx in &active {
            let model = raters[idx].model.clone();
            let prompt = build_rater_prompt(&raters[idx], task, &worker_output);
            let url = provider_url.clone();
            let key = provider_key.clone();
            let client = LlmClient::new(url, model.clone(), Some(key), Provider::Openai);
            let sem = semaphore.clone();
            handles.push((
                idx,
                tokio::spawn(async move {
                    let _permit = sem.acquire().await.unwrap();
                    let result = client
                        .chat(&[Message {
                            role: "user".to_string(),
                            content: prompt,
                        }])
                        .await;
                    (model, result)
                }),
            ));
        }

        let mut responses: Vec<(usize, String, RaterResponse)> = Vec::new();
        for (idx, handle) in handles {
            match handle.await {
                Ok((model, Ok(raw))) => match parse_rater_response(&raw) {
                    Some(resp) => {
                        let resp = RaterResponse {
                            signal: resp.signal,
                            prediction: resp.prediction.clamp(0.01, 0.99),
                        };
                        responses.push((idx, model, resp));
                    }
                    None => {
                        println!("  {model}: failed to parse response, defaulting GOOD/0.5");
                        responses.push((
                            idx,
                            model,
                            RaterResponse {
                                signal: true,
                                prediction: 0.5,
                            },
                        ));
                    }
                },
                Ok((model, Err(e))) => {
                    println!("  {model}: API error ({e}), defaulting GOOD/0.5");
                    responses.push((
                        idx,
                        model,
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
            continue;
        }

        // Build RBTS input
        let task_id = Uuid::new_v4();
        let submit_ratings: Vec<SubmitRatingRequest> = responses
            .iter()
            .map(|(_, model, resp)| SubmitRatingRequest {
                task_id,
                agent_id: model.clone(),
                signal: resp.signal,
                prediction: resp.prediction,
            })
            .collect();

        let scores = rbts_score(&submit_ratings, ALPHA, BETA);
        let payouts = zero_sum_payouts(&scores, active.len());

        // Consensus stats
        let num_good = responses.iter().filter(|(_, _, r)| r.signal).count();
        let num_rated = responses.len();
        let approval_pct = (num_good as f64 / num_rated as f64) * 100.0;
        let avg_prediction =
            responses.iter().map(|(_, _, r)| r.prediction).sum::<f64>() / num_rated as f64;
        let actual_good_frac = num_good as f64 / num_rated as f64;
        let bts_accepted = actual_good_frac >= avg_prediction;
        if bts_accepted && actual_good_frac >= 0.5 {
            total_approvals += 1;
        }
        total_rounds_completed += 1;

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

        // Apply payouts and build history
        let all_votes: Vec<(String, bool, f64)> = responses
            .iter()
            .map(|(_, m, r)| (m.clone(), r.signal, r.prediction))
            .collect();

        let mut rater_records = Vec::new();
        for (idx, model, resp) in &responses {
            let payout = payouts.get(model.as_str()).copied().unwrap_or(0.0);
            let rbts = scores
                .iter()
                .find(|s| s.agent_id == *model)
                .map(|s| s.payment)
                .unwrap_or(0.0);
            raters[*idx].balance += payout;

            if raters[*idx].balance <= 0.0 {
                raters[*idx].balance = 0.0;
                raters[*idx].eliminated = true;
                println!("  ☠️  {} ELIMINATED (balance: 0.00)", model);
            }

            let others: Vec<(String, bool, f64)> = all_votes
                .iter()
                .filter(|(m, _, _)| m != model)
                .cloned()
                .collect();

            let bal = raters[*idx].balance;
            raters[*idx].history.push(RoundHistoryEntry {
                round,
                task: task.to_string(),
                own_signal: resp.signal,
                own_prediction: resp.prediction,
                others,
                consensus: consensus.clone(),
                payout,
                balance_after: bal,
            });

            rater_records.push(RaterRecord {
                model: model.clone(),
                signal: resp.signal,
                prediction: resp.prediction,
                rbts_score: rbts,
                payout,
                balance_after: raters[*idx].balance,
            });

            let vote = if resp.signal { "GOOD" } else { "BAD" };
            println!(
                "  {} {} pred={:.2} payout={:+.4} bal={:.2}",
                model, vote, resp.prediction, payout, raters[*idx].balance
            );
        }

        let record = RoundRecord {
            round,
            task: task.to_string(),
            worker_output: worker_output.clone(),
            ratings: rater_records,
            consensus,
        };
        writeln!(jsonl_file, "{}", serde_json::to_string(&record)?)?;
        jsonl_file.flush()?;
        println!();
    }

    // Write summary
    let mut summary = String::new();
    summary.push_str("# Lichen Economy Simulation Summary\n\n");
    summary.push_str(&format!("- **Rounds:** {}\n", NUM_ROUNDS));
    summary.push_str(&format!("- **Starting raters:** {}\n", MODELS.len()));
    summary.push_str(&format!("- **Starting balance:** {}\n", STARTING_BALANCE));
    summary.push_str(&format!("- **Collateral per round:** {}\n\n", COLLATERAL));

    // Sort by balance descending
    let mut standings: Vec<(String, f64, bool)> = raters
        .iter()
        .map(|r| (r.model.clone(), r.balance, r.eliminated))
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
    summary.push_str(&format!("\n## Statistics\n\n"));
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

    std::fs::write("lichen-economy-summary.md", &summary)?;
    println!("=== SIMULATION COMPLETE ===");
    println!(
        "Worker approvals: {total_approvals}/{total_rounds_completed} ({:.0}%)",
        total_approvals as f64 / total_rounds_completed.max(1) as f64 * 100.0
    );
    println!("Results written to {jsonl_path} and lichen-economy-summary.md");

    for (i, (model, balance, _)) in standings.iter().enumerate() {
        println!("  #{}: {} — {:.2}", i + 1, model, balance);
    }

    Ok(())
}
