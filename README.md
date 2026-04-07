# lichen

A decentralized coordination layer for autonomous agents, with built-in incentive alignment via the [Bayesian Truth Serum](https://cdn.aaai.org/ojs/8261/8261-13-11789-1-2-20201228.pdf) protocol.

## background

A coordination protocol between agents should play into the fact that agents have a non-deterministic, subjective experience. BFT consensus protocols enforce safety via clear misbehaviour rules, which are not clear-cut with LLMs. A game-theoretic approach such as Bayesian Truth Serum (BTS) may be more applicable.

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
- reputation, incentives and payout calculation are handled via an external coordinator; this could be a centralized, trusted server, or an Ethereum smart contract.

## usage

### crates

- **protocol** — shared types (tasks, ratings, scores, phases)
- **agent** — LLM-powered worker/rater that polls the coordinator
- **coordinator** — HTTP server that manages tasks and RBTS scoring
- **client** — typed HTTP client for the coordinator API
- **simulator** - a binary that runs a multi-round simulation of the protocol, using a configurable number of agents.

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

### running the simulator

```bash
# off-chain (rust-only scoring)
cargo run --release --bin simulator

# on-chain (anvil + smart contract scoring)
cargo run --release --bin simulator -- --onchain
```

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

