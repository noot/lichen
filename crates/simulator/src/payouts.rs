use std::collections::HashMap;

use crate::COLLATERAL;

pub(crate) fn zero_sum_payouts(
    scores: &[protocol::ScoreResult],
    active_count: usize,
) -> HashMap<String, f64> {
    let pool = active_count as f64 * COLLATERAL;
    let n = scores.len() as f64;

    let min_score = scores
        .iter()
        .map(|s| s.payment)
        .fold(f64::INFINITY, f64::min);
    let shifted: Vec<f64> = scores.iter().map(|s| s.payment - min_score).collect();
    let total: f64 = shifted.iter().sum();

    let mut payouts = HashMap::new();
    if total < 1e-12 {
        let equal_share = pool / n;
        for s in scores {
            payouts.insert(s.agent_id.clone(), equal_share);
        }
    } else {
        for (i, s) in scores.iter().enumerate() {
            payouts.insert(s.agent_id.clone(), shifted[i] / total * pool);
        }
    }
    payouts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(agent_id: &str, payment: f64) -> protocol::ScoreResult {
        protocol::ScoreResult {
            agent_id: agent_id.to_string(),
            payment,
        }
    }

    fn assert_pool_fully_distributed(payouts: &HashMap<String, f64>, active_count: usize) {
        let pool = active_count as f64 * COLLATERAL;
        let total: f64 = payouts.values().sum();
        assert!(
            (total - pool).abs() < 1e-10,
            "pool ({pool}) minus payouts ({total}) must be zero, got {}",
            pool - total
        );
    }

    fn assert_all_non_negative(payouts: &HashMap<String, f64>) {
        for (id, &payout) in payouts {
            assert!(payout >= 0.0, "{id} got negative payout: {payout}");
        }
    }

    #[test]
    fn payouts_sum_to_pool() {
        let scores = vec![score("alice", 2.0), score("bob", -1.0), score("carol", 0.5)];
        let payouts = zero_sum_payouts(&scores, 3);
        assert_pool_fully_distributed(&payouts, 3);
    }

    #[test]
    fn all_payouts_are_non_negative() {
        let scores = vec![
            score("alice", 5.0),
            score("bob", -3.0),
            score("carol", 1.0),
            score("dave", -10.0),
        ];
        let payouts = zero_sum_payouts(&scores, 4);
        assert_all_non_negative(&payouts);
        assert_pool_fully_distributed(&payouts, 4);
    }

    #[test]
    fn every_participant_gets_a_payout_entry() {
        let scores = vec![
            score("alice", 5.0),
            score("bob", -3.0),
            score("carol", 1.0),
            score("dave", -2.0),
        ];
        let payouts = zero_sum_payouts(&scores, 4);
        for s in &scores {
            assert!(
                payouts.contains_key(&s.agent_id),
                "missing payout for {}",
                s.agent_id
            );
        }
    }

    #[test]
    fn larger_pool_scales_payouts() {
        let scores = vec![score("a", 10.0), score("b", -5.0), score("c", 3.0)];
        let small = zero_sum_payouts(&scores, 2);
        let large = zero_sum_payouts(&scores, 10);
        let ratio = large["a"] / small["a"];
        assert!(
            (ratio - 5.0).abs() < 1e-10,
            "5x pool should produce 5x payouts, got ratio {ratio}"
        );
    }

    #[test]
    fn equal_scores_split_pool_evenly() {
        let scores = vec![score("alice", 1.0), score("bob", 1.0), score("carol", 1.0)];
        let payouts = zero_sum_payouts(&scores, 3);
        let expected = 3.0 * COLLATERAL / 3.0;
        for (id, &payout) in &payouts {
            assert!(
                (payout - expected).abs() < 1e-12,
                "{id} should get equal share {expected}, got {payout}"
            );
        }
        assert_pool_fully_distributed(&payouts, 3);
    }

    #[test]
    fn higher_score_gets_higher_payout() {
        let scores = vec![score("high", 10.0), score("low", -10.0)];
        let payouts = zero_sum_payouts(&scores, 2);
        assert!(
            payouts["high"] > payouts["low"],
            "higher scorer should get more: high={}, low={}",
            payouts["high"],
            payouts["low"]
        );
        assert_pool_fully_distributed(&payouts, 2);
    }

    #[test]
    fn lowest_scorer_gets_zero() {
        let scores = vec![score("good", 100.0), score("bad", -100.0)];
        let payouts = zero_sum_payouts(&scores, 2);
        assert!(
            payouts["bad"].abs() < 1e-12,
            "lowest scorer should get 0, got {}",
            payouts["bad"]
        );
        let pool = 2.0 * COLLATERAL;
        assert!(
            (payouts["good"] - pool).abs() < 1e-12,
            "highest scorer should get entire pool ({pool}), got {}",
            payouts["good"]
        );
    }

    #[test]
    fn active_count_larger_than_participants() {
        let scores = vec![score("alice", 3.0), score("bob", -1.0)];
        let active_count = 10;
        let payouts = zero_sum_payouts(&scores, active_count);

        assert_pool_fully_distributed(&payouts, active_count);
        assert_all_non_negative(&payouts);
        assert_eq!(payouts.len(), 2);
    }

    #[test]
    fn single_participant_gets_entire_pool() {
        let scores = vec![score("solo", 5.0)];
        let payouts = zero_sum_payouts(&scores, 1);
        assert_eq!(payouts.len(), 1);
        let pool = COLLATERAL;
        assert!(
            (payouts["solo"] - pool).abs() < 1e-12,
            "single participant should get entire pool ({pool}), got {}",
            payouts["solo"]
        );
    }

    #[test]
    fn many_participants_exhaust_pool() {
        let scores: Vec<protocol::ScoreResult> = (0..50)
            .map(|i| score(&format!("agent_{i}"), (i as f64 - 25.0) * 0.3))
            .collect();
        let payouts = zero_sum_payouts(&scores, 50);
        assert_pool_fully_distributed(&payouts, 50);
        assert_all_non_negative(&payouts);
        assert_eq!(payouts.len(), 50);
    }
}
