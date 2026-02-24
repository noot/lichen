use protocol::{ScoreResult, SubmitRatingRequest};

/// Compute RBTS scores for a set of rating responses.
///
/// For each agent:
///   - Information score: fraction of peers whose signal matches theirs
///   - Prediction score: QPS(their prediction, actual fraction who said "good")
///   - Payment = alpha * information_score + beta * prediction_score
///
/// Returns zero payments if fewer than 2 responses are provided.
pub(crate) fn rbts_score(
    responses: &[SubmitRatingRequest],
    alpha: f64,
    beta: f64,
) -> Vec<ScoreResult> {
    if responses.len() < 2 {
        return responses
            .iter()
            .map(|r| ScoreResult {
                agent_id: r.agent_id.clone(),
                payment: 0.0,
            })
            .collect();
    }

    let num_good = responses.iter().filter(|r| r.signal).count();
    let actual_good_frac = num_good as f64 / responses.len() as f64;

    responses
        .iter()
        .enumerate()
        .map(|(i, resp)| {
            // compare with all peers (not self) and average
            let peers: Vec<usize> = (0..responses.len()).filter(|&j| j != i).collect();
            let information_score = peers
                .iter()
                .map(|&j| {
                    if resp.signal == responses[j].signal {
                        1.0
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / peers.len() as f64;

            let prediction_score = qps(resp.prediction, actual_good_frac);

            ScoreResult {
                agent_id: resp.agent_id.clone(),
                payment: alpha * information_score + beta * prediction_score,
            }
        })
        .collect()
}

/// Quadratic Prediction Score.
/// QPS(p, x) = 2px + 2(1-p)(1-x) - p² - (1-p)²
/// where p = prediction, x = actual frequency of "good".
fn qps(prediction: f64, actual: f64) -> f64 {
    2.0 * prediction * actual + 2.0 * (1.0 - prediction) * (1.0 - actual)
        - prediction * prediction
        - (1.0 - prediction) * (1.0 - prediction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_response(agent_id: &str, signal: bool, prediction: f64) -> SubmitRatingRequest {
        SubmitRatingRequest {
            task_id: Uuid::new_v4(),
            agent_id: agent_id.to_string(),
            signal,
            prediction,
        }
    }

    #[test]
    fn qps_perfect_prediction_returns_max_score() {
        // predict 1.0, actual 1.0 → max score = 1.0
        assert!((qps(1.0, 1.0) - 1.0).abs() < 1e-10);
        // predict 0.0, actual 0.0 → max score = 1.0
        assert!((qps(0.0, 0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn qps_worst_prediction_returns_min_score() {
        // predict 1.0, actual 0.0 → min score = -1.0
        assert!((qps(1.0, 0.0) - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn honest_majority_gets_non_negative_payments() {
        let responses = vec![
            make_response("a", true, 0.8),
            make_response("b", true, 0.7),
            make_response("c", true, 0.9),
            make_response("d", false, 0.6),
        ];
        let scores = rbts_score(&responses, 1.0, 1.0);
        assert_eq!(scores.len(), 4);
        // all should have non-negative payments when mostly honest
        for s in &scores {
            assert!(
                s.payment >= 0.0,
                "{} had negative payment: {}",
                s.agent_id,
                s.payment
            );
        }
    }

    #[test]
    fn single_liar_scores_lower_than_honest_agents() {
        // 4 honest say "bad", 1 liar says "good"
        let responses = vec![
            make_response("honest1", false, 0.2),
            make_response("honest2", false, 0.2),
            make_response("honest3", false, 0.1),
            make_response("honest4", false, 0.2),
            make_response("liar", true, 0.8),
        ];

        let scores = rbts_score(&responses, 1.0, 1.0);
        let liar_payment = scores
            .iter()
            .find(|s| s.agent_id == "liar")
            .unwrap()
            .payment;
        let honest_avg: f64 = scores
            .iter()
            .filter(|s| s.agent_id != "liar")
            .map(|s| s.payment)
            .sum::<f64>()
            / 4.0;

        // honest agents should score higher than the liar
        assert!(honest_avg > liar_payment);
    }
}
