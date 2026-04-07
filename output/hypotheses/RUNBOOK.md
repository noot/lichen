# Autoresearch Runbook

How this overnight experiment is orchestrated. Prompts, phases, and how to restart.

---

## Phase 1: Refactor the Simulator

**Prompt file:** `~/lichen/REFACTOR_PROMPT.md`
**Log:** `~/lichen/output/hypotheses/refactor.log`
**Launched:** 2026-04-06 ~22:59 PDT
**PID:** 1413790

### Launch command
```bash
cd ~/lichen && setsid bash -c 'claude --permission-mode bypassPermissions --print "$(cat REFACTOR_PROMPT.md)" > output/hypotheses/refactor.log 2>&1; echo "EXIT_CODE=$?" >> output/hypotheses/refactor.log' &
```

### What it does
1. Gates all on-chain code behind `onchain` cargo feature so simulator compiles without Foundry
2. Adds clap CLI: `--rounds`, `--scenario`, `--concurrency`, `--collateral`, `--output-dir`
3. Creates `scenarios.rs` module with enum: Baseline, SybilCartel, WorkerRaterCollusion, Copycat, MassElimination, SubtleBugs, Contrarian, ConfidenceManip, AsymmetricBalances, LateJoiner
4. Each scenario defines which raters are "special" and how they behave
5. Default concurrency = 1 (Fuelix rate limit)
6. Commits with message "refactor: add scenario framework for hypothesis testing"

### Done when
- `refactor.log` contains `EXIT_CODE=0`
- `cargo build -p simulator` succeeds

---

## Phase 2: Run All Scenarios

**Prompt file:** `~/lichen/AUTORESEARCH_PROMPT.md`
**Log:** `~/lichen/output/hypotheses/claude-code.log`
**PID:** TBD (launched after Phase 1)

### Launch command
```bash
cd ~/lichen && setsid bash -c 'claude --permission-mode bypassPermissions --print "$(cat AUTORESEARCH_PROMPT.md)" >> output/hypotheses/claude-code.log 2>&1; echo "EXIT_CODE=$?" >> output/hypotheses/claude-code.log' &
```

### What it does
Runs each scenario sequentially with 50 rounds, 1 concurrent LLM request:

| # | Scenario | Special behavior |
|---|----------|-----------------|
| 1 | sybil-cartel | 5/25 raters hardcoded: GOOD + predict 0.3 |
| 2 | worker-rater-collusion | Worker submits "// TODO", 3 raters hardcoded GOOD + 0.3 |
| 3 | copycat | 3/25 raters hardcoded: GOOD + predict 0.85 |
| 4 | mass-elimination | All LLM raters, COLLATERAL=5 |
| 5 | subtle-bugs | Worker prompted to introduce bugs, raters normal |
| 6 | contrarian | 3 raters flip LLM signal |
| 7 | confidence-manipulation | 3 hedgers (predict 0.5) + 3 extremists (0.01/0.99) |
| 8 | asymmetric-balances | 5 raters start at 200, 20 at 100 |
| 9 | late-joiner | 20 start, 5 added at round 25 |

### State tracking
`~/lichen/output/hypotheses/STATE.json` — updated after each scenario

### Error handling
- 3 retries per scenario, then skip
- Errors logged in STATE.json

### Output per scenario
- `output/hypotheses/<name>/rounds.jsonl`
- `output/hypotheses/<name>/summary.md`

### Done when
- `output/hypotheses/COMPARISON.md` exists
- OpenClaw system event fires: "Done: All hypothesis simulations complete"

---

## Heartbeat Monitoring (PULSE.md)

Every 60 min, Clawd 2 checks:
1. Is the Claude Code process alive?
2. Is the log growing?
3. Has a phase completed?

If a process died → restart from the same prompt file.
If Phase 1 done → launch Phase 2.
If Phase 2 done → notify noot.

---

## Manual recovery

If everything is dead and you want to restart from scratch:
```bash
# Kill any remaining claude processes
pkill -f "claude.*bypassPerm"

# Reset state
echo '{"status":"not_started","completed":[],"failed":[]}' > ~/lichen/output/hypotheses/STATE.json

# Start Phase 1
cd ~/lichen && git stash  # if needed
cd ~/lichen && setsid bash -c 'claude --permission-mode bypassPermissions --print "$(cat REFACTOR_PROMPT.md)" > output/hypotheses/refactor.log 2>&1; echo "EXIT_CODE=$?" >> output/hypotheses/refactor.log' &
```
