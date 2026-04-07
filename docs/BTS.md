# Bayesian Truth Serum

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
