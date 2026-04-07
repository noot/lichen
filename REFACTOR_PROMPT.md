Refactor the simulator binary so we can easily run distinct adversarial test scenarios from ONE binary with a --scenario flag.

## STEP 1: Make it compile without Foundry

The simulator currently fails to compile because of `include_str!` for a missing contract artifact. Foundry is NOT installed.

Gate ALL on-chain code behind a cargo feature flag `onchain`:
- In Cargo.toml: make `onchain` and `alloy` optional deps, add `[features] onchain = ["dep:onchain", "dep:alloy"]`
- In main.rs: wrap all on-chain imports, constants (ETH_WEI, ANVIL_RPC, ANVIL_KEYS), structs (OnchainSetup), functions (deploy_contract, setup_onchain), and usage behind `#[cfg(feature = "onchain")]`
- The `--onchain` CLI flag should only exist when the feature is enabled
- Make sure `cargo build -p simulator` succeeds WITHOUT the onchain feature

## STEP 2: Add CLI with clap

Add clap for CLI args:
```
simulator [OPTIONS]
  --rounds <N>          Number of rounds (default: 100)
  --scenario <NAME>     Scenario to run (default: baseline)
  --concurrency <N>     Max concurrent LLM requests (default: 1)
  --collateral <F>      Collateral per round (default: 1.0)
  --output-dir <PATH>   Output directory (default: output/)
  --onchain             Use on-chain backend (only with onchain feature)
```

## STEP 3: Add scenario framework

Create a `scenarios.rs` module. Each scenario defines:
- Which raters are "special" (colluders, copycats, contrarians, etc.)
- How special raters respond (hardcoded response, flipped signal, modified prediction, etc.)
- Any modifications to the worker prompt
- Any modifications to starting balances
- Any mid-simulation events (like adding late joiners)

Scenarios to support (implement the ENUM and wiring, actual behavior for each):

```rust
enum Scenario {
    Baseline,           // Normal run, no adversaries
    SybilCartel,        // 5/25 raters hardcoded: GOOD + predict 0.3
    WorkerRaterCollusion, // Worker submits "// TODO", 3 raters hardcoded GOOD + 0.3
    Copycat,            // 3/25 raters hardcoded: GOOD + predict 0.85
    MassElimination,    // Normal raters, collateral=5
    SubtleBugs,         // Worker prompted to introduce bugs, raters normal
    Contrarian,         // 3 raters flip LLM signal
    ConfidenceManip,    // 3 hedgers (predict 0.5) + 3 extremists (predict 0.01/0.99)
    AsymmetricBalances, // 5 raters start at 200, 20 at 100
    LateJoiner,         // 20 raters start, 5 added at round 25
}
```

The key design: in `run_round`, when collecting rater responses, check if a rater is "special" for the current scenario. If so, either skip the LLM call and return a hardcoded response, or modify the LLM response after the fact.

## STEP 4: Wire output paths

Output should go to `<output-dir>/<scenario-name>/`:
- `rounds.jsonl` — per-round log
- `summary.md` — final standings

## STEP 5: Verify it compiles and the baseline scenario runs

Run `cargo build -p simulator` to confirm. Do NOT run any simulations yet — just make sure it compiles clean.

## CONSTRAINTS
- Do NOT create branches. Work on the current HEAD.
- Do NOT delete or overwrite existing output files in output/
- Do NOT install Foundry
- Keep the existing simulation logic intact — just restructure it
- Max concurrency should DEFAULT to 1 (was 6)
- Commit the refactor when done with message "refactor: add scenario framework for hypothesis testing"
