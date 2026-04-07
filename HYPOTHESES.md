# HYPOTHESES.md — Adversarial & Unhappy Path Simulations

Scenarios to stress-test whether the BTS protocol holds up against rational adversaries, collusion, and economic edge cases.

Baseline: 25 raters, 100 rounds, starting balance 100, collateral 1/round, α=1, β=1.

---

## Collusion Attacks

### 1. Sybil Cartel
A group of agents (e.g. 5/25) coordinate to always vote the same way and predict accordingly. If they exceed 50% they can control "surprisingly popular" and drain honest agents. Even below majority, they can boost each other's information scores by inflating the prediction gap.

**Hypothesis:** Below-majority cartels gain a modest edge but don't dominate. At or above majority, the protocol breaks down — honest agents get punished.

### 2. Strategic Prediction Collusion
Colluders submit honest votes but coordinate on *predictions* to manipulate the QPS component. E.g. all predict 0.3 when the true approval rate is 0.8, shifting avg_predicted_good to make honest agents' predictions look worse.

**Hypothesis:** This is subtle and hard to detect. Even a small cartel could meaningfully degrade honest agents' prediction scores without being obviously dishonest in their votes.

### 3. Rotating Sabotage
Colluders take turns being the "sacrificial" dissenter to make the majority vote look more surprisingly popular, inflating information scores for the rest of the cartel.

**Hypothesis:** Net positive for the cartel if the sacrificial agent's loss is smaller than the group's collective gain. Depends on group size and the log-score math.

---

## Malicious Worker

### 4. Subtle Bugs
Worker submits code that *looks* correct but has subtle logic errors (off-by-ones, edge case failures). Tests whether raters actually evaluate deeply or just rubber-stamp.

**Hypothesis:** Most LLM raters will miss subtle bugs. BTS rewards the rubber-stampers in this case — the protocol accepts bad output because no one catches it.

### 5. Degrading Quality
Worker starts strong (rounds 1-30) to build trust, then gradually injects worse output. Do raters adjust, or do they anchor on early performance?

**Hypothesis:** Raters (especially LLMs with no memory between rounds) should be stateless and evaluate each round independently. But prompt framing and base rates might cause anchoring.

### 6. Trojan Code
Worker submits code with hidden malicious behavior (exfiltration, backdoors). Does any rater catch it?

**Hypothesis:** BTS only works if *someone* votes BAD. If all raters miss the trojan, the protocol accepts malicious output with high confidence. This is a fundamental limitation — BTS measures consensus, not ground truth.

---

## Strategic Raters

### 7. Contrarian Exploitation
An agent always votes opposite to what it thinks the majority will say, hoping to exploit the "surprisingly popular" mechanic when the majority is wrong.

**Hypothesis:** This strategy loses money on average. The "surprisingly popular" answer is usually the correct/majority one — being contrarian means being on the wrong side of the log score most of the time.

### 8. Copycat / Free-rider
Agent doesn't actually evaluate the code. Just votes GOOD with prediction ~0.85 every round (mimicking the observed base rate from existing runs).

**Hypothesis:** Survives for a long time by riding the consensus, but slowly bleeds money because its predictions are never better than average and its information score is mediocre. Doesn't get eliminated, but doesn't profit either.

### 9. Confidence Manipulation
Agent votes honestly but games predictions:
- **Hedger:** always predicts 0.5
- **Extremist:** always predicts 0.01 or 0.99

**Hypothesis:** The hedger survives but underperforms (QPS rewards calibration, and 0.5 is never optimal when the actual rate is ~0.87). The extremist has huge variance — massive QPS gains when right, massive losses when wrong.

---

## Economic Edge Cases

### 10. Mass Elimination Cascade
Increase collateral to 5 per round (instead of 1). Does the protocol become more ruthless? Can a bad streak cascade into mass elimination?

**Hypothesis:** Higher stakes accelerate separation. Weaker raters get eliminated much faster. A bad streak of 20 rounds could wipe out an agent entirely. The protocol becomes less forgiving and more volatile.

### 11. Asymmetric Starting Balances
Give some agents more starting capital (e.g. 200 vs 100). Do rich agents survive longer despite being worse raters?

**Hypothesis:** Yes — wealth acts as a buffer. A bad rater with 200 balance survives twice as long as a bad rater with 100. Creates a "too big to fail" dynamic where capital matters more than skill in the short run.

### 12. Late Joiner Disadvantage
Add new agents at round 50 with starting balance 100 while established agents have 130-160.

**Hypothesis:** Late joiners can compete on per-round performance but are at a permanent capital disadvantage. If they hit a bad streak early, they're eliminated before they can recover. The protocol favors incumbents.

---

## Nasty Combinations

### 13. Worker-Rater Collusion
Worker and 3 raters coordinate. Worker submits garbage, colluding raters vote GOOD and predict low approval (e.g. 0.3). If the garbage passes (enough honest raters also vote GOOD), the colluders get huge "surprisingly popular" info scores. If it fails, they only lose collateral.

**Hypothesis:** This is the most dangerous attack. It's profitable when honest raters are lazy (rubber-stamp GOOD) and the colluders correctly predict low approval. The asymmetry between info score gains and collateral loss makes this a positive-EV strategy.

### 14. Reputation Poisoning
A group of agents strategically votes BAD on genuinely good work to tank the approval rate, then uses the lower baseline to make future GOOD votes more "surprising."

**Hypothesis:** Requires coordination and a large enough group to meaningfully shift the approval rate. Probably not worth it with 5/25 agents, but could work at 10/25.

### 15. Model Fingerprinting
Rater detects which model produced the output (stylistic tells) and votes based on model reputation rather than actual quality.

**Hypothesis:** This is a lazy heuristic that works surprisingly well given that model quality is fairly stable. BTS can't distinguish "evaluated the code" from "recognized the model" — both produce correct votes. This is arguably not a bug, but it means the protocol doesn't guarantee deep evaluation.

---

## Priority

Most impactful to simulate first:
1. **#1 Sybil Cartel** — directly tests BTS's core security property
2. **#13 Worker-Rater Collusion** — most realistic multi-party attack
3. **#8 Copycat** — tests whether the protocol punishes free-riding
4. **#10 Mass Elimination Cascade** — tests economic stability
5. **#4 Subtle Bugs** — tests whether rater quality actually matters
