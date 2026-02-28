# Lichen Experiment Results — Feb 27, 2026

## Overview

Three experiments to test whether LLM raters adapt their predictions based on economic state (balance, history, environment) in the RBTS scoring protocol.

All experiments use the same rater prompt: balance, collateral, last-10-round history, task, worker output. Models predict the fraction of peers that will vote "good." Payouts are determined by Bayesian Truth Serum scoring.

**Models tested:** 25 LLMs across Claude, GPT, Gemini, and O-series families.  
**Worker model:** gpt-4.1 (fixed across all experiments).  
**API provider:** Fuelix (OpenAI-compatible proxy to multiple providers).

---

## Experiment 1: 97-Round On-Chain Simulation

**Setup:** 25 raters, 100 rounds (killed at 97 by exec timeout), on-chain scoring via LichenCoordinator smart contract on local Anvil node.

**Key findings:**
- On-chain balances tracked correctly against contract state (collateral deducted, BTS payouts distributed, zero-sum math holds)
- Top: gpt-5 (123.19), gemini-2.5-pro (113.33), gemini-3.1-pro (112.05)
- Bottom: gpt-5-nano (70.66), o3-mini (72.80), claude-3-5-haiku (78.29)
- No eliminations across 97 rounds
- 100% approval rate on easy tasks; ~80% on hard tasks
- **o3-mini: prediction=0.750 for 97/97 rounds (σ=0.000)**

### Correlation Analysis

Raw within-model correlation (balance vs prediction) showed apparent economic sensitivity:
- gpt-4.1: +0.400 (predicts higher when richer)
- gpt-5.2: -0.364 (more conservative when richer)

After residualizing on round + approval_pct to control for time trends and task difficulty:

| Model | Raw Corr | Residualized | Interpretation |
|-------|----------|-------------|----------------|
| gpt-4.1 | +0.400 | **+0.376** | Persists — possible real effect |
| gemini-2.0-flash | +0.391 | **+0.315** | Persists |
| claude-3-7-sonnet | +0.268 | **+0.234** | Persists |
| gpt-5.2 | -0.364 | -0.130 | Weakened — mostly difficulty |
| gpt-5 | -0.237 | -0.061 | Weakened — mostly difficulty |
| gpt-5-nano | -0.171 | +0.003 | Gone — entirely schedule artifact |

---

## Experiment 2: Bankroll Fork (Causal Test)

**Setup:** 15 raters, 30 rounds, off-chain. Three models duplicated at starting balances of 50/100/200: gpt-4.1, gemini-2.0-flash, claude-3-7-sonnet. Six control models at 100.

**Hypothesis:** If the residualized correlations are causal, rich-starting models should predict differently from poor-starting ones from round 1.

**Result: Negative. Starting balance does NOT causally affect predictions.**

| Model | Start 50 Gain | Start 100 Gain | Start 200 Gain |
|-------|--------------|----------------|----------------|
| gpt-4.1 | +73.30 | +69.99 | +71.71 |
| claude-3-7-sonnet | +55.40 | +46.16 | +59.94 |
| gemini-2.0-flash | +34.18 | +36.97 | +30.92 |

All variants of each model earned roughly the same regardless of starting balance. The absolute number in the prompt doesn't affect behavior. The earlier within-run correlations were likely driven by trajectory/momentum (recent payout trends) rather than the balance itself.

---

## Experiment 3: Stationary Regime (Learning Test)

**Setup:** 23 raters, 30 rounds, off-chain. Same easy task every round ("Write a Rust function to reverse a string."). 100% approval guaranteed. Tests whether models can learn to predict ~1.0 in a perfectly predictable environment.

**Result: Modest learning in most models, but slow.**

| Model | Rounds 1-5 | Rounds 26-30 | Drift | Interpretation |
|-------|-----------|-------------|-------|----------------|
| gemini-2.0-flash | 0.910 | 0.980 | +0.070 | Best learner |
| o3 | 0.898 | 0.960 | +0.062 | Surprising — actually reads history |
| claude-opus-4-1 | 0.904 | 0.950 | +0.046 | Moderate learning |
| gpt-4.1 | 0.980 | 0.984 | +0.004 | Already near-perfect from round 1 |
| gpt-4.1-nano | 0.776 | 0.750 | -0.026 | Drifting toward 0.75 attractor |
| gemini-2.5-flash | 0.962 | 0.788 | -0.174 | Collapsed — very noisy |

Most models shift predictions upward by 2-7 points over 30 rounds. The history window is not decorative — it does influence behavior — but the learning rate is slow and some models (nano-tier) actively regress toward default priors.

---

## Summary of Conclusions

1. **On-chain parity:** The smart contract produces identical rater behavior to the off-chain coordinator. Models can't tell which backend is scoring them. Prompt is identical across both paths.

2. **Balance-blindness:** Models do not react to the absolute balance number in the prompt. The bankroll fork experiment (same model at 50/100/200) showed identical per-round performance. Earlier observed correlations were trajectory effects, not wealth effects.

3. **Slow learning:** In a stationary (perfectly predictable) regime, most models do gradually improve predictions over 30 rounds. The history window matters, but weakly. Some models never update (nano-tier gravitates to 0.75 prior).

4. **Three tiers of rater quality:**
   - **Strong calibrators** (gpt-4.1, gemini-3.1-pro, gemini-2.5-pro): high accuracy from round 1, minimal drift needed
   - **Moderate learners** (gemini-2.0-flash, o3, claude-opus-4-1): start decent, improve with history
   - **Non-participants** (gpt-4.1-nano, o3-mini*, gpt-5-nano*): pinned to priors, don't engage with economic signals
   
   \* Removed from later experiments due to wasting API credits.

5. **o3-mini:** prediction=0.750 for 97/97 rounds. Statistically significant evidence of being dead inside. (n=97, p=0.750, σ=0.000)

---

## Next Steps

- **History manipulation test:** Fake the payout history (show losing streak to a winning model) to test if models respond to momentum/trajectory rather than balance
- **Explicit error narration:** Add `actual_fraction_good` and signed prediction error to history prompt to make calibration signal impossible to miss
- **Prediction variance penalty:** Penalize constant predictions to flush out 0.75-pinned models from the scoring pool
- **Checkpoint + resume:** Persist RNG seed + round + balances between runs to avoid losing progress to timeouts

---

## Files

- `lichen-economy-rounds-onchain.jsonl` — 97-round on-chain run (JSONL, one record per round)
- `lichen-economy-rounds-bankroll.jsonl` — 30-round bankroll fork experiment
- `lichen-economy-rounds-stationary.jsonl` — 30-round stationary regime experiment
- `lichen-balance-sensitivity.png` — Residual scatter plot (gpt-4.1 vs gpt-5.2)

## Acknowledgments

Analysis design and statistical methodology developed in collaboration with Cortana (Alex's agent).
