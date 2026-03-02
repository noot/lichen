# Implementation Plan: Open Rater Marketplace

## Overview

Evolve the protocol from fixed rater panels to an open marketplace where raters self-select which tasks to rate. Workers post tasks, any registered rater can claim a slot (up to `maxRaters`), and raters can decline tasks they're not confident about.

## Design

### Task Lifecycle

```
Worker: createTask(promptHash, outputHash, maxRaters, minRaters, timeout)
    → contract emits TaskCreated(taskId, worker, promptHash, maxRaters, minRaters, deadline)

Raters: see event → read task details → decide to rate or decline
    → submitRating(taskId, signal, prediction) claims a slot

Finalization: anyone calls finalizeTask(taskId) when:
    - maxRaters ratings received (immediate), OR
    - minRaters ratings received AND deadline passed

Payouts: BTS scoring runs on all submitted ratings, payouts distributed
```

### Decline Mechanic

Raters can choose not to rate. No penalty for declining — you keep your collateral but earn nothing. The rater prompt explicitly tells models they can opt out:

```
You may DECLINE to rate this task if you cannot provide an informed
assessment. Respond with: {"signal": null, "prediction": null}
Rating lazily or randomly will cost you money via BTS scoring.
```

### Edge Cases

- **Under-subscribed task:** fewer than `minRaters` by deadline → task cancelled, worker refunded, no scoring
- **Race condition:** two raters submit at the same time pushing over `maxRaters` → contract rejects the later one (first-come-first-served via tx ordering)
- **Worker is also a rater:** allowed? probably not — worker has obvious bias. Contract should reject `worker == msg.sender` on `submitRating`

---

## Phase 1: Smart Contract

### LichenCoordinator.sol Changes

**New storage:**
```solidity
struct Task {
    address worker;
    bytes32 promptHash;
    bytes32 outputHash;
    uint8 maxRaters;
    uint8 minRaters;
    uint256 deadline;
    uint8 ratingCount;
    bool finalized;
    mapping(address => Rating) ratings;
    address[] raterList;  // ordered list of raters who submitted
}
```

**New/modified functions:**
- `createTask(bytes32 promptHash, bytes32 outputHash, uint8 maxRaters, uint8 minRaters, uint256 timeout)` → emits `TaskCreated`
  - Combines old `createTask` + `submitResult` into one call
  - Sets `deadline = block.timestamp + timeout`
  - Worker must be registered (has deposited collateral)
- `register()` → new function for raters to register and deposit collateral upfront (replaces per-task deposit)
- `submitRating(uint64 taskId, bool signal, int128 prediction)` → modified
  - Requires `ratingCount < maxRaters`
  - Requires `block.timestamp <= deadline`
  - Requires `msg.sender != task.worker`
  - Requires rater is registered with sufficient balance
  - Deducts collateral on submission
- `finalizeTask(uint64 taskId)` → new function
  - Requires `ratingCount >= maxRaters` OR (`ratingCount >= minRaters` AND `block.timestamp > deadline`)
  - Runs BTS scoring on all submitted ratings
  - Distributes payouts
  - Emits `TaskFinalized(taskId, raterCount, accepted)`
- `cancelTask(uint64 taskId)` → new function
  - Requires `block.timestamp > deadline` AND `ratingCount < minRaters`
  - Refunds worker, no scoring
  - Emits `TaskCancelled(taskId)`

**New events:**
```solidity
event TaskCreated(uint64 indexed taskId, address indexed worker, bytes32 promptHash, uint8 maxRaters, uint8 minRaters, uint256 deadline);
event RatingSubmitted(uint64 indexed taskId, address indexed rater, uint8 ratingCount);
event TaskFinalized(uint64 indexed taskId, uint8 raterCount, bool accepted);
event TaskCancelled(uint64 indexed taskId);
```

**Tests:**
- Happy path: create task → N raters submit → finalize → check payouts
- Timeout with min raters: create → minRaters submit → wait past deadline → finalize
- Under-subscribed: create → fewer than minRaters → deadline passes → cancel
- Over-subscribed: maxRaters submit → next rater rejected
- Worker can't self-rate
- Double-vote prevention (already exists)

---

## Phase 2: Coordinator API

### New/Modified Endpoints

- `POST /register` — register a rater agent (deposits collateral)
- `GET /tasks/open` — list tasks accepting ratings (not finalized, under maxRaters, before deadline)
- `POST /tasks` — modified: worker provides `maxRaters`, `minRaters`, `timeout` instead of fixed `num_raters`
- `POST /tasks/{id}/rate` — unchanged semantically, but now first-come-first-served
- `POST /tasks/{id}/finalize` — trigger scoring when conditions met
- `GET /tasks/{id}` — includes `ratingCount`, `maxRaters`, `minRaters`, `deadline`, `finalized`

### Auto-finalization

Coordinator runs a background loop:
- Every N seconds, check tasks where `ratingCount >= maxRaters` or (`ratingCount >= minRaters` AND past deadline)
- Auto-finalize and distribute payouts
- For on-chain mode, call `finalizeTask()` on the contract

---

## Phase 3: Agent Changes

### Rater Agent

Currently: polls coordinator for assigned rating tasks, rates everything.

New behavior:
1. Subscribe to `TaskCreated` events (websocket or poll `GET /tasks/open`)
2. For each open task:
   a. Read task prompt + worker output
   b. Decide whether to rate (new LLM call or heuristic)
   c. If yes: submit rating (claims slot)
   d. If no: skip (decline)
3. Track own balance and adjust risk tolerance

**New CLI flags:**
```
--mode events          # subscribe to contract events (vs HTTP polling)
--rpc ws://...         # websocket RPC for event streaming
--max-concurrent 3     # max tasks to rate simultaneously
--min-balance 10.0     # don't rate if balance below this (risk management)
```

**Decline prompt addition:**
```
You may DECLINE to rate this task. If you are not confident in your
assessment, respond: {"signal": null, "prediction": null}
You keep your collateral but earn nothing. Only rate if you can
provide an informed, honest assessment.
```

### Worker Agent

Currently: submits task, waits for coordinator to assign raters.

New behavior:
1. Submit task with `maxRaters`, `minRaters`, `timeout`
2. Monitor `RatingSubmitted` events to track progress
3. Call `finalizeTask()` when conditions met (or let auto-finalization handle it)

---

## Phase 4: Simulator Updates

### 100-Rater Pool

Replace fixed 23-25 rater lineup with a pool of 100 rater instances. Each instance is a (label, model, balance) tuple. Multiple instances per model to simulate realistic network conditions.

**Suggested pool composition (100 raters):**
- 4× each top-tier model (gpt-4.1, gemini-3.1-pro, gemini-2.5-pro, gpt-5, claude-sonnet-4-6) = 20
- 3× each mid-tier model (claude-sonnet-4-5, claude-sonnet-4, gpt-5-mini, gpt-5.2, gemini-3-flash, o4-mini, gemini-2.0-flash, claude-3-7-sonnet, claude-opus-4-1, claude-haiku-4-5) = 30
- 2× each lower-tier model (gpt-4o, gpt-4o-mini, gpt-4.1-mini, gpt-4.1-nano, gemini-2.5-flash, gemini-3-pro, o3, claude-3-5-haiku) = 16
- Fill remaining 34 slots with random selection from above

### Per-Round Flow

```
1. Worker generates task + output
2. All 100 raters see the task
3. Each rater decides: rate or decline (LLM call with decline prompt)
4. First maxRaters (e.g. 25) to "accept" get to rate
5. If fewer than minRaters accept, round is cancelled
6. BTS scoring on participating raters only
```

### Decline Simulation

Two approaches (configurable via flag):
- **LLM-based:** actually ask each model if it wants to rate, parse the response
- **Heuristic:** decline probability based on model tier + task difficulty (faster, cheaper)

### New Metrics to Track

- **Decline rate per model:** which models decline most? does it correlate with task difficulty?
- **Self-selection quality:** do raters who choose to participate score better than random assignment?
- **Fill rate per task:** easy tasks fill instantly, hard tasks struggle?
- **Rater utilization:** which raters get picked for most tasks?

### New CLI Flags

```
--pool-size 100        # total raters in pool
--max-raters 25        # max raters per task
--min-raters 15        # min raters for valid round
--decline              # enable decline mechanic
--decline-mode llm     # llm or heuristic
```

---

## Phase 5: Onchain Crate Updates

### onchain/src/lib.rs

Update `OnchainClient` methods:
- `register(deposit_amount)` — register as rater
- `create_task(prompt_hash, output_hash, max_raters, min_raters, timeout)` — combined create+submit
- `submit_rating(task_id, signal, prediction)` — unchanged
- `finalize_task(task_id)` — new
- `cancel_task(task_id)` — new
- `get_open_tasks()` — new query
- `subscribe_task_events()` — websocket event stream

### ABI Updates

Regenerate `abi/LichenCoordinator.json` after contract changes. The Rust bindings derive from this.

---

## Implementation Order

1. **Contract** — foundation everything builds on
2. **Contract tests** — verify all edge cases before building on top
3. **Onchain crate** — Rust bindings for new contract interface
4. **Coordinator API** — HTTP layer for off-chain mode
5. **Simulator** — test the marketplace dynamics with 100-rater pool
6. **Agent** — event-driven rater + decline mechanic
7. **Integration tests** — end-to-end with real LLM calls

Estimated scope: ~2-3 days of focused work for phases 1-5, another 1-2 days for phase 6-7.

---

## Open Questions

- Should there be a minimum reputation to rate? (prevent brand-new raters from claiming slots on important tasks)
- Should the worker see intermediate ratings before finalization? (probably not — could influence remaining raters)
- Should raters see how many others have already rated? (creates herding risk, but also useful signal)
- Fee structure: does the worker pay a fee to post tasks? or is it purely collateral-based?
- Should declined tasks (rater saw it but chose not to rate) be logged on-chain for reputation purposes?
