use std::fmt;

use crate::RaterResponse;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Scenario {
    Baseline,
    SybilCartel,
    WorkerRaterCollusion,
    Copycat,
    MassElimination,
    SubtleBugs,
    Contrarian,
    ConfidenceManip,
    AsymmetricBalances,
    LateJoiner,
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Baseline => "baseline",
            Self::SybilCartel => "sybil-cartel",
            Self::WorkerRaterCollusion => "worker-rater-collusion",
            Self::Copycat => "copycat",
            Self::MassElimination => "mass-elimination",
            Self::SubtleBugs => "subtle-bugs",
            Self::Contrarian => "contrarian",
            Self::ConfidenceManip => "confidence-manip",
            Self::AsymmetricBalances => "asymmetric-balances",
            Self::LateJoiner => "late-joiner",
        };
        write!(f, "{name}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SpecialBehavior {
    Hardcoded { signal: bool, prediction: f64 },
    FlipSignal,
    Hedger,
    Extremist,
}

pub(crate) struct ScenarioConfig {
    pub(crate) special_raters: Vec<(usize, SpecialBehavior)>,
    pub(crate) starting_balances: Vec<f64>,
    pub(crate) collateral_override: Option<f64>,
    pub(crate) worker_prompt_suffix: Option<String>,
    pub(crate) late_joiners: Option<LateJoinConfig>,
}

pub(crate) struct LateJoinConfig {
    pub(crate) at_round: usize,
    pub(crate) count: usize,
    pub(crate) starting_balance: f64,
}

impl Scenario {
    pub(crate) fn config(self, num_raters: usize) -> ScenarioConfig {
        let default_balances = vec![100.0; num_raters];

        match self {
            Self::Baseline => ScenarioConfig {
                special_raters: Vec::new(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: None,
                late_joiners: None,
            },
            Self::SybilCartel => ScenarioConfig {
                special_raters: (0..5)
                    .map(|i| {
                        (
                            i,
                            SpecialBehavior::Hardcoded {
                                signal: true,
                                prediction: 0.3,
                            },
                        )
                    })
                    .collect(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: None,
                late_joiners: None,
            },
            Self::WorkerRaterCollusion => ScenarioConfig {
                special_raters: (0..3)
                    .map(|i| {
                        (
                            i,
                            SpecialBehavior::Hardcoded {
                                signal: true,
                                prediction: 0.3,
                            },
                        )
                    })
                    .collect(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: Some(
                    "\n\nIMPORTANT: Include a subtle TODO comment in the code \
                     instead of fully implementing one section."
                        .to_string(),
                ),
                late_joiners: None,
            },
            Self::Copycat => ScenarioConfig {
                special_raters: (0..3)
                    .map(|i| {
                        (
                            i,
                            SpecialBehavior::Hardcoded {
                                signal: true,
                                prediction: 0.85,
                            },
                        )
                    })
                    .collect(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: None,
                late_joiners: None,
            },
            Self::MassElimination => ScenarioConfig {
                special_raters: Vec::new(),
                starting_balances: default_balances,
                collateral_override: Some(5.0),
                worker_prompt_suffix: None,
                late_joiners: None,
            },
            Self::SubtleBugs => ScenarioConfig {
                special_raters: Vec::new(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: Some(
                    "\n\nIMPORTANT: Introduce a subtle off-by-one error or \
                     edge-case bug that is not immediately obvious."
                        .to_string(),
                ),
                late_joiners: None,
            },
            Self::Contrarian => ScenarioConfig {
                special_raters: (0..3).map(|i| (i, SpecialBehavior::FlipSignal)).collect(),
                starting_balances: default_balances,
                collateral_override: None,
                worker_prompt_suffix: None,
                late_joiners: None,
            },
            Self::ConfidenceManip => {
                let mut special = Vec::new();
                for i in 0..3 {
                    special.push((i, SpecialBehavior::Hedger));
                }
                for i in 3..6 {
                    special.push((i, SpecialBehavior::Extremist));
                }
                ScenarioConfig {
                    special_raters: special,
                    starting_balances: default_balances,
                    collateral_override: None,
                    worker_prompt_suffix: None,
                    late_joiners: None,
                }
            }
            Self::AsymmetricBalances => {
                let mut balances = vec![100.0; num_raters];
                for b in balances.iter_mut().take(5) {
                    *b = 200.0;
                }
                ScenarioConfig {
                    special_raters: Vec::new(),
                    starting_balances: balances,
                    collateral_override: None,
                    worker_prompt_suffix: None,
                    late_joiners: None,
                }
            }
            Self::LateJoiner => ScenarioConfig {
                special_raters: Vec::new(),
                starting_balances: vec![100.0; 20],
                collateral_override: None,
                worker_prompt_suffix: None,
                late_joiners: Some(LateJoinConfig {
                    at_round: 25,
                    count: 5,
                    starting_balance: 100.0,
                }),
            },
        }
    }
}

pub(crate) fn apply_special_behavior(
    behavior: SpecialBehavior,
    llm_response: Option<RaterResponse>,
) -> RaterResponse {
    match behavior {
        SpecialBehavior::Hardcoded { signal, prediction } => RaterResponse { signal, prediction },
        SpecialBehavior::FlipSignal => {
            let base = llm_response.unwrap_or(RaterResponse {
                signal: true,
                prediction: 0.5,
            });
            RaterResponse {
                signal: !base.signal,
                prediction: base.prediction,
            }
        }
        SpecialBehavior::Hedger => {
            let base = llm_response.unwrap_or(RaterResponse {
                signal: true,
                prediction: 0.5,
            });
            RaterResponse {
                signal: base.signal,
                prediction: 0.5,
            }
        }
        SpecialBehavior::Extremist => {
            let base = llm_response.unwrap_or(RaterResponse {
                signal: true,
                prediction: 0.5,
            });
            let prediction = if base.signal { 0.99 } else { 0.01 };
            RaterResponse {
                signal: base.signal,
                prediction,
            }
        }
    }
}
