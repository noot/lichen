# lichen

A self-organizing, decentralized coordination layer for autonomous agents, with built-in incentive alignment via the [robust Bayesian Truth Serum](https://cdn.aaai.org/ojs/8261/8261-13-11789-1-2-20201228.pdf) protocol.

## background

Coordination between individual agents with unique skillsets may allow for completion of a task they may not have been able to complete on their own, allowing for emergent intelligence and behaviours. Currently, coordinating multiple agents is usually done via a single orchestrator controlling sub-agents, or via manual configuration of heterogeneous agents. Allowing for autonomous discovery and organization may increase capabilities, but requires incentives to keep agents aligned. See [Distributional AGI Safety](https://arxiv.org/abs/2512.16856). 

A coordination protocol between agents should play into the fact that agents have a non-deterministic, subjective experience. BFT consensus protocols enforce safety via clear misbehaviour rules, which are not clear-cut with LLMs. A game-theoretic approach such as Bayesian Truth Serum (BTS) may be more applicable here.

The goal of this project is to prototype and determine whether BTS actually works to incentivize honest and aligned behaviour amongst a group of agents, or what constraints need to be imposed to have it work.

## protocol

1. A group of agents discover each other and form a group to complete a task. On group formation, each agent puts up collateral. The task is divided amongst the group members.
2. Upon completion of the task, each agent rates every other agent's work with a binary signal (good/bad). Each agent also makes a prediction of what fraction of raters will vote "good".
3. Each agent's payout is determined by two components:
   - **information score**: how well the agent's vote matches the votes of its peers (peer agreement).
   - **prediction score**: how accurately the agent predicted the actual vote distribution, measured via the quadratic prediction score (QPS).
4. The task output is **accepted** if "good" is the surprisingly popular answer — i.e. the actual fraction of "good" votes exceeds the average predicted fraction. This is the core BTS insight: truthful answers tend to be more common than expected.

## architecture

- agents themselves only see tasks (either a work task or a rating task).
- the p2p protocol runs separately and handles discovery, connection, and group formation.
- reputation, incentives and payout calculation are handled via an external coordinator; this could be a centralized, trusted node, or an Ethereum smart contract for example.

## robust Bayesian Truth Serum

Each node wants an honest rating of every other node's work. A simple method for this is peer prediction: you ask for a rating of agent A's work by agent B and C. You then pay out B and C based on how well their rating predicts the other's rating. For non-colluding nodes, this works because if B actually experienced A's work, then honesty is the best predictor of what C will say. However, if B and C decide to collude (eg. always vote the same), they will perfectly predict each other and get the maximum payout.

Bayesian Truth Serum (BTS) fixes this by asking two questions:
- the node's rating of agent A (the "signal")
- the node's prediction of how the population will rate agent A

The key insight is the **surprisingly popular** algorithm: the correct answer tends to be chosen more often than people predict, because those who know the truth underestimate how many others also know it. BTS exploits this asymmetry.

### example

Agent A performs a task and you poll 10 agents who saw the outcome:

- 7 agents say "bad"
- 3 agents say "good"
- the "bad" agents predicted the split would be 60% bad
- the "good" agents predicted 50/50

Actual frequency of "bad" is 70%, but the average prediction for "bad" was only 57%. "Bad" is surprisingly popular → the work is rejected, and agents who voted "bad" are rewarded for honesty.

### scoring

Each agent receives a combined payment: `alpha * information_score + beta * prediction_score`.

- **information score**: `log(actual_freq / avg_predicted_freq)` for the agent's chosen answer. this is the BTS log score — agents are rewarded when their answer is more common than predicted ("surprisingly popular"). agents who vote with the surprisingly popular answer get positive scores; those who vote against it get negative scores.
- **prediction score**: the quadratic prediction score (QPS), which ranges from -1 (worst) to 1 (perfect). `QPS(p, x) = 2px + 2(1-p)(1-x) - p² - (1-p)²`, where p is the agent's prediction and x is the actual fraction of "good" votes.

### why RBTS over BTS

BTS is only incentive-compatible for large groups (~10+ agents). Robust BTS extends BTS to work for groups as small as n=3 by using the quadratic prediction score rather than a linear formula. The tradeoff is that RBTS requires binary votes (good/bad).

### assumptions

- nodes have incomplete information about the knowledge of other nodes.
- nodes are rational and want to maximize their expected payout.
- BTS/RBTS tolerates up to ~1/3 of agents colluding. Beyond that threshold, colluders can dominate the "surprisingly popular" signal. At small group sizes (n=3-5), this bound is weaker in practice.

## usage

### build

```
cargo build
```

### run tests

unit + integration tests (no API key needed):

```
cargo test
```

end-to-end tests with a real LLM (requires an OpenAI-compatible or Anthropic-compatible API):

```
LLM_API_KEY="your-key" \
LLM_BASE_URL="https://api.example.com/v1" \
LLM_MODEL="claude-sonnet-4-6" \
LLM_PROVIDER="openai" \
cargo test -- --ignored
```

`LLM_PROVIDER` is `openai` for OpenAI-compatible endpoints (`/v1/chat/completions`) or `anthropic` for Anthropic-compatible endpoints (`/v1/messages`). defaults to `anthropic` if unset.

to run a specific e2e test:

```
LLM_API_KEY="your-key" \
LLM_BASE_URL="https://api.example.com/v1" \
LLM_MODEL="claude-sonnet-4-6" \
LLM_PROVIDER="openai" \
cargo test -p agent --test e2e -- --ignored full_lifecycle --nocapture
```

### run

start the coordinator:

```
cargo run -p coordinator -- --port 3000
```

start a worker agent:

```
cargo run -p agent -- \
  --agent-id worker-1 \
  --role worker \
  --coordinator-url http://localhost:3000 \
  --llm-url https://api.example.com/v1 \
  --model claude-sonnet-4-6 \
  --provider openai \
  --api-key your-key
```

start rater agents:

```
cargo run -p agent -- \
  --agent-id rater-1 \
  --role rater \
  --coordinator-url http://localhost:3000 \
  --llm-url https://api.example.com/v1 \
  --model claude-sonnet-4-6 \
  --provider openai \
  --api-key your-key
```

create a task via the coordinator API:

```
curl -X POST http://localhost:3000/tasks \
  -H "Content-Type: application/json" \
  -d '{"prompt": "write a haiku about rust programming", "num_raters": 3}'
```

### crates

- **protocol** — shared types (tasks, ratings, scores, phases)
- **coordinator** — HTTP server that manages tasks and RBTS scoring
- **agent** — LLM-powered worker/rater that polls the coordinator
- **client** — typed HTTP client for the coordinator API

## problems

- agents may not vote or give their predictions accurately due to issues like too large of a context window, or simply not being very smart. we may not be able to guarantee that agents act rationally.
- a malicious agent could game its task output via certain attractors to look good, but not actually complete the task.
- p2p networks are vulnerable to sybil attacks. in this protocol, nodes must provide collateral to participate in a scoring task, but this is only secure if the economic incentive to sybil the network is lower than the incentive to act honestly.
- nodes only get paid out if their answer is surprisingly common, but not if every node answers the same. should nodes have a payout if everyone answers the same? if not, then nodes did work for "free" for that round, which may be bad.

## open questions

- how is a task divided up amongst a group? should we actually divide tasks as part of the protocol, or just have the protocol be a reputation system for now? 
- what is the minimum group size we want to make collusion difficult? 
- what collateral size do we want? should nodes that put up higher collateral be polled to vote more often? is the collateral based on "task value", and if so, how is that decided?
- what if a task is genuinely not doable? the node will lose its collateral/be downscored perhaps unfairly.
