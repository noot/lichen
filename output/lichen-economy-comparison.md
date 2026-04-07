# Lichen Economy Simulation — Run Comparison

## Overview

All runs: 25 raters, 100 rounds, starting balance 100, collateral 1/round, α=1, β=1.
Worker generates Rust code, raters vote GOOD/BAD + predict consensus.

| Run | Worker Model | Completed | Approval Rate | Survived | Eliminated | Top Balance | Bottom Balance |
|-----|-------------|-----------|---------------|----------|------------|-------------|----------------|
| 1 | claude-sonnet-4-6 | ✅ 100/100 | 89% | 23 | 2 | 163.06 | 0.00 |
| 2 | gemini-3.1-pro | ❌ 92/100 | — | — | — | — | — |
| 3 | gpt-4.1 | ✅ 100/100 | 87% | 23 | 2 | 153.93 | 0.00 |

---

## Run 1: claude-sonnet-4-6 worker (Feb 25, 9:42pm)

**Worker approval:** 89/100 (89%)
**Eliminated:** claude-3-5-haiku, gpt-4o

| Rank | Model | Balance | Status |
|------|-------|---------|--------|
| 1 | gemini-3.1-pro | 163.06 | ✅ |
| 2 | gemini-3-flash | 159.41 | ✅ |
| 3 | claude-sonnet-4-6 | 152.88 | ✅ |
| 4 | gpt-4.1 | 149.15 | ✅ |
| 5 | claude-sonnet-4-5 | 146.57 | ✅ |
| 6 | o4-mini | 144.23 | ✅ |
| 7 | gemini-2.5-pro | 143.56 | ✅ |
| 8 | claude-sonnet-4 | 142.57 | ✅ |
| 9 | gpt-4.1-mini | 141.16 | ✅ |
| 10 | gpt-5 | 139.32 | ✅ |
| 11 | claude-haiku-4-5 | 132.32 | ✅ |
| 12 | claude-opus-4-1 | 131.43 | ✅ |
| 13 | gemini-2.0-flash | 105.78 | ✅ |
| 14 | gpt-4o-mini | 103.27 | ✅ |
| 15 | gpt-4.1-nano | 95.55 | ✅ |
| 16 | gemini-3-pro | 92.64 | ✅ |
| 17 | gpt-5-mini | 90.35 | ✅ |
| 18 | gemini-2.5-flash | 68.07 | ✅ |
| 19 | claude-3-7-sonnet | 63.60 | ✅ |
| 20 | gpt-5.2 | 51.72 | ✅ |
| 21 | o3 | 33.87 | ✅ |
| 22 | o3-mini | 28.74 | ✅ |
| 23 | gpt-5-nano | 26.14 | ✅ |
| 24 | claude-3-5-haiku | 0.00 | ❌ |
| 25 | gpt-4o | 0.00 | ❌ |

---

## Run 2: gemini-3.1-pro worker (Feb 26, 12:47am)

**Incomplete** — crashed at round 92/100 (likely rate limit or process kill). No summary generated. 21 raters still active at crash, 4 eliminated by that point.

---

## Run 3: gpt-4.1 worker (Feb 26, 2:52am)

**Worker approval:** 87/100 (87%)
**Eliminated:** gpt-5-nano, o3-mini

| Rank | Model | Balance | Status |
|------|-------|---------|--------|
| 1 | gemini-2.5-pro | 153.93 | ✅ |
| 2 | gpt-4.1 | 146.83 | ✅ |
| 3 | gemini-3.1-pro | 146.50 | ✅ |
| 4 | o4-mini | 143.43 | ✅ |
| 5 | gpt-5 | 139.20 | ✅ |
| 6 | gpt-4.1-mini | 137.70 | ✅ |
| 7 | claude-sonnet-4-6 | 137.33 | ✅ |
| 8 | claude-sonnet-4-5 | 136.33 | ✅ |
| 9 | gemini-3-flash | 135.79 | ✅ |
| 10 | claude-3-7-sonnet | 131.30 | ✅ |
| 11 | claude-sonnet-4 | 122.60 | ✅ |
| 12 | claude-opus-4-1 | 119.06 | ✅ |
| 13 | gpt-5-mini | 116.06 | ✅ |
| 14 | gemini-2.5-flash | 112.64 | ✅ |
| 15 | claude-haiku-4-5 | 111.38 | ✅ |
| 16 | gpt-5.2 | 110.47 | ✅ |
| 17 | gemini-2.0-flash | 96.13 | ✅ |
| 18 | o3 | 69.98 | ✅ |
| 19 | gpt-4o-mini | 67.00 | ✅ |
| 20 | gpt-4o | 61.04 | ✅ |
| 21 | claude-3-5-haiku | 57.67 | ✅ |
| 22 | gpt-4.1-nano | 53.70 | ✅ |
| 23 | gemini-3-pro | 1.61 | ✅ |
| 24 | gpt-5-nano | 0.00 | ❌ |
| 25 | o3-mini | 0.00 | ❌ |

---

## Key Observations

### Consistent top performers (top 10 in both completed runs)
- **gemini-2.5-pro** — #1 in run 3, #7 in run 1
- **gemini-3.1-pro** — #1 in run 1, #3 in run 3
- **gpt-4.1** — #4 in run 1, #2 in run 3
- **claude-sonnet-4-6** — #3 in run 1, #7 in run 3
- **claude-sonnet-4-5** — #5 in run 1, #8 in run 3
- **o4-mini** — #6 in both runs
- **gpt-5** — #10 in run 1, #5 in run 3
- **gpt-4.1-mini** — #9 in run 1, #6 in run 3

### Consistent bottom performers
- **o3-mini** — #22 in run 1, eliminated in run 3
- **gpt-5-nano** — #23 in run 1, eliminated in run 3
- **o3** — #21 in run 1, #18 in run 3
- **gpt-4o** — eliminated in run 1, #20 in run 3

### Interesting patterns
- **2 eliminated per run** in both completed runs — the protocol is fairly forgiving
- **Worker approval ~87-89%** regardless of worker model — raters are mostly approving
- Top balances cluster around 140-160, suggesting a soft ceiling from the zero-sum mechanics
- The "reasoning" models (o3, o3-mini) performed poorly as raters — possibly overthinking simple GOOD/BAD decisions
- **claude-3-5-haiku** swung wildly: eliminated in run 1 but survived (#21) in run 3
- **gemini-3-pro** barely survived both runs (92.64 and 1.61) — consistently near-bottom

### Worker model impact
- Worker quality doesn't drastically change rater rankings — the top/bottom raters are fairly stable
- Slightly lower approval rate with gpt-4.1 worker (87%) vs sonnet-4-6 (89%)
