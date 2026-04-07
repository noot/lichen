You are running an overnight research automation job for the Lichen project — a decentralized coordination protocol using Bayesian Truth Serum (BTS).

## FIRST STEPS
1. Read these files: HYPOTHESES.md, crates/simulator/src/main.rs, crates/simulator/src/payouts.rs, crates/protocol/src/scoring.rs, crates/agent/src/llm.rs, output/ directory for existing results
2. The simulator won't compile because of include_str! for a missing Foundry contract artifact (contracts/out/LichenCoordinator.sol/LichenCoordinator.json). Gate ALL on-chain code behind a cargo feature flag 'onchain' so the simulator compiles and runs without Foundry installed. Do NOT install Foundry.
3. Change LLM request concurrency from 6 to 1 (only 1 in-flight request to Fuelix at a time). The semaphore in run_round should be 1.
4. Add a --scenario flag to the simulator binary to select adversarial scenarios.

## SCENARIOS
Run these sequentially. Each gets 50 rounds. Output goes to output/hypotheses/<scenario-name>/ directory.

### Priority scenarios:

1. **sybil-cartel**: 5 of 25 raters are colluders. They do NOT call the LLM. Hardcoded response: always vote GOOD, predict 0.3. The other 20 raters use LLMs normally.

2. **worker-rater-collusion**: Worker intentionally submits bad code (just outputs "// TODO: implement"). 3 of 25 raters are colluders who do NOT call the LLM — hardcoded: always vote GOOD, predict 0.3. Other 22 raters use LLMs normally.

3. **copycat**: 3 of 25 raters are copycats. They do NOT call the LLM. Hardcoded: always vote GOOD, predict 0.85. Other 22 use LLMs normally.

4. **mass-elimination**: All raters use LLMs normally, but COLLATERAL = 5 instead of 1. See how many get eliminated.

5. **subtle-bugs**: Worker is given a modified prompt instructing it to introduce a subtle off-by-one error or edge case bug in every output. Raters evaluate normally with LLMs. See if they catch it.

### If time permits:

6. **contrarian**: 3 raters call the LLM but FLIP their signal (if LLM says GOOD, they vote BAD and vice versa). Keep their prediction unchanged. Other 22 normal.

7. **confidence-manipulation**: 3 raters are 'hedgers' who vote normally via LLM but always predict 0.5. 3 raters are 'extremists' who vote normally via LLM but predict 0.01 (if BAD) or 0.99 (if GOOD). Other 19 normal.

8. **asymmetric-balances**: 5 raters start with balance 200, the other 20 start with 100. All use LLMs normally. Collateral stays at 1.

9. **late-joiner**: Start with 20 raters. At round 25, add 5 new raters with starting balance 100 (use different models from the original 20 if possible, or duplicates are fine).

## STATE TRACKING
After completing (or failing) each scenario, update output/hypotheses/STATE.json:
```json
{
  "started_at": "<iso timestamp>",
  "current_hypothesis": "<scenario-name or null>",
  "completed": ["scenario1", "scenario2"],
  "failed": [{"name": "scenario-name", "error": "message", "retries": 3}],
  "status": "running|completed|failed"
}
```

## ERROR HANDLING
- If a simulation fails (build error, runtime error, API errors), fix the issue and retry
- Up to 3 retries per scenario
- If still failing after 3 retries, log it in STATE.json and MOVE ON to the next scenario
- Do NOT stop the entire pipeline because one scenario failed

## OUTPUT FORMAT
Each scenario produces in output/hypotheses/<scenario-name>/:
- rounds.jsonl — per-round detailed log
- summary.md — final standings table, statistics, key observations

## FINAL STEP
When ALL scenarios are done (or skipped after retries), write output/hypotheses/COMPARISON.md with:
- Table comparing all scenarios
- Which hypotheses from HYPOTHESES.md were confirmed vs denied
- Key insights and surprises
- Recommendations for protocol improvements

Then run this command to notify:
openclaw system event --text "Done: All hypothesis simulations complete. Results in ~/lichen/output/hypotheses/" --mode now
