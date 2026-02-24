# lichen

A self-organizing, decentralized coordination layer for autonomous agents, with built-in incentive alignment via the [robust Bayesian Truth Serum](https://cdn.aaai.org/ojs/8261/8261-13-11789-1-2-20201228.pdf) protocol.

## background

Coordination between individual agents with unique skillsets may allow for completion of a task they may not have been able to complete on their own, allowing for emergent intelligence and behaviours. Currently, coordinating multiple agents is usually done via a single orchestator controlling sub-agents, or via manual configuration of heterogenous agents. Allowing for automonous discovery and organization may increase capabilities, but requires incentives to keep agents aligned. See [Distributional AGI Safety](https://arxiv.org/abs/2512.16856). 

## goals

- allow agents to autonomously discover each other and organize in a decentralized manner to complete a task
- enable effective group formation via p2p discovery and reputation
- incentivize agents to act honestly and usefully

## protocol

1. A group of agents discover each other and form a group to complete a task. On group formation, each agent puts up collateral. The task is divided amongst the group members.
2. Upon completion of the task, each agent rates every other agent's work by answering if the other agent completed its sub-task (yes/no). Each agent also makes a prediction as to what the vote split will be, eg. 60% of nodes will vote yes.
3. The RBTS formula is used to compare the average prediction vs the actual frequency, which pays out nodes which had a "surprisingly common" rating from the points pool; ie the. Agents whose prediction matched actual get 0 payout, while agents whose rating was less common than expected lose their collateral.

## architecture

- agents themselves only see tasks (either a work task or a rating task).
- the p2p protocol runs separately and handles discovery, connection, and group formation.
- reputation, incentives and payout calculation are handled via an external coordinator; this could be a centralized, trusted node, or an Ethereum smart contract for example.

## robust Bayesian Truth Serum

Each node wants an honest rating of every other node's work. A simple method for this is peer prediction; you ask for a rating of agent A's work by agent B and C. You then pay out B and C based on how well their rating predicts the other's rating. For non-colluding nodes, this works because if B actually experienced A's work, then honesty is the best predictor of what C will say. However, if B and C decide to collude (eg. always vote the same), they will perfectly predict each other and get the maximum payout.

Bayesian Truth Serum (BTS) fixes this by asking two questions:
- the node's rating of agent A
- the node's prediction of how the population will rate agent A

The scoring formula pays out answers that are "surprisingly common", ie. the answer appears more frequently than the average of the predictions said it would.

For example:

Agent A performs a task and you poll 10 agents who saw the outcome of the task as to whether it was completed or not.

- 7 agents say no
- 3 agents say yes

The "no" agents said the split would be 60% no. The "yes" agents said the split would be 50/50. Actual frequency of "no" is 70% but the average prediction for "no" was 57%. Agents who voted "no" get a payout.

Assumptions:

- nodes have incomplete information about the knowledge of other nodes. BTS fails if over >1/3 nodes are colluding.
- nodes are rational and want to maximize their expected payout.

BTS is only incentive-compatible for a large enough number of agents (~10+); Robust BTS extends BTS to work for groups with n >= 3 by using a quadratic prediction score formula rather than a linear formula. However, with RBTS, the votes must be binary values.

## problems

- agents may not vote or give their predictions accurately due to issues like too large of a context window, or simply not being very smart. we may not be able to guarantee that agents act rationally.
- a malicious agent could game its task output via certain attractors to look good, but not actually complete the task.
- p2p networks are vulnerable to sybil attacks. in this protocol, nodes must provide collateral to participate in a scoring task, but this is only secure if the economic incentive to sybil the network is lower than the incentive to act honestly.

## open questions

- how is a task divided up amongst a group?
- what is the minimum group size we want to make collusion difficult? 
- what collateral size do we want? should nodes that put up higher collateral be polled to vote more often? is the collateral based on "task value", and if so, how is that decided?
- what if a task is genuinely not doable? the node will lose its collateral/be downscored perhaps unfairly.