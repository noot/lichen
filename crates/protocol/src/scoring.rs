use crate::{ScoreResult, SubmitRatingRequest};

/// small epsilon to avoid log(0)
const EPS: f64 = 1e-10;

/// Compute RBTS scores for a set of rating responses.
///
/// For each agent:
///   - Information score: log(actual_freq / avg_predicted_freq) for the agent's
///     chosen signal. this is the BTS "surprisingly popular" log score — agents
///     are rewarded when their answer is more common than predicted.
///   - Prediction score: QPS(their prediction, actual fraction who said "good")
///   - Payment = alpha * information_score + beta * prediction_score
///
/// Returns zero payments if fewer than 2 responses are provided.
pub fn rbts_score(responses: &[SubmitRatingRequest], alpha: f64, beta: f64) -> Vec<ScoreResult> {
    if responses.len() < 2 {
        return responses
            .iter()
            .map(|r| ScoreResult {
                agent_id: r.agent_id.clone(),
                payment: 0.0,
            })
            .collect();
    }

    let n = responses.len() as f64;
    let num_good = responses.iter().filter(|r| r.signal).count();
    let actual_good_frac = (num_good as f64 / n).clamp(EPS, 1.0 - EPS);
    let actual_bad_frac = 1.0 - actual_good_frac;

    // average predicted fraction of "good" across all agents
    let avg_predicted_good =
        (responses.iter().map(|r| r.prediction).sum::<f64>() / n).clamp(EPS, 1.0 - EPS);
    let avg_predicted_bad = 1.0 - avg_predicted_good;

    responses
        .iter()
        .map(|resp| {
            // log score: log(actual_freq / avg_predicted_freq) for the chosen answer.
            // agents who voted with the "surprisingly popular" answer get positive scores.
            let information_score = if resp.signal {
                (actual_good_frac / avg_predicted_good).ln()
            } else {
                (actual_bad_frac / avg_predicted_bad).ln()
            };

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
    fn surprisingly_popular_answer_gets_positive_information_score() {
        // 3 agents vote "good" and predict 0.5, 1 votes "bad" and predicts 0.5.
        // actual good = 0.75, avg predicted good = 0.5.
        // "good" is surprisingly popular → log(0.75/0.5) > 0.
        // "bad" is surprisingly unpopular → log(0.25/0.5) < 0.
        let responses = vec![
            make_response("a", true, 0.5),
            make_response("b", true, 0.5),
            make_response("c", true, 0.5),
            make_response("d", false, 0.5),
        ];
        let scores = rbts_score(&responses, 1.0, 0.0); // information score only
        let good_voter = scores.iter().find(|s| s.agent_id == "a").unwrap();
        let bad_voter = scores.iter().find(|s| s.agent_id == "d").unwrap();
        assert!(
            good_voter.payment > 0.0,
            "surprisingly popular voter should have positive score"
        );
        assert!(
            bad_voter.payment < 0.0,
            "surprisingly unpopular voter should have negative score"
        );
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
