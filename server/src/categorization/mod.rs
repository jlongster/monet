use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::payee_resolution::normalize_payee;

pub mod run_fixture;

const TRANSFER_OUTCOME_ID: &str = "__transfer__";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CategoryAccountType {
    Income,
    Expense,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceAccountType {
    Asset,
    Liability,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryAccount {
    pub id: String,
    pub name: String,
    pub account_type: CategoryAccountType,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizationExample {
    pub payee: String,
    pub imported_category: Option<String>,
    #[serde(default)]
    pub date: String,
    pub amount: i64,
    pub source_account_type: SourceAccountType,
    #[serde(default)]
    pub source_account_id: String,
    pub counterpart_account_id: String,
    #[serde(default)]
    pub transfer: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizationQuery {
    pub payee: String,
    pub imported_category: Option<String>,
    #[serde(default)]
    pub date: String,
    pub amount: i64,
    pub source_account_type: SourceAccountType,
    #[serde(default)]
    pub source_account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizationEvaluation {
    pub name: String,
    #[serde(flatten)]
    pub query: CategorizationQuery,
    pub expected_account_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CategorizationFixtures {
    pub accounts: Vec<CategoryAccount>,
    pub history: Vec<CategorizationExample>,
    pub evaluations: Vec<CategorizationEvaluation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizationNeighbor {
    pub payee: String,
    pub account_id: String,
    pub account_name: String,
    pub similarity: f64,
    pub recency: f64,
    pub vote: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryCandidate {
    pub account_id: String,
    pub account_name: String,
    pub vote: f64,
    pub neighbor_count: usize,
    pub agreement: f64,
    pub best_similarity: f64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CategorizationMethod {
    ExactHistory,
    LlmFallback,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorizationResolution {
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub potential_transfer: bool,
    pub method: CategorizationMethod,
    pub reason: String,
    pub agreement: f64,
    pub support: f64,
    pub margin: f64,
    pub candidates: Vec<CategoryCandidate>,
    pub neighbors: Vec<CategorizationNeighbor>,
}

#[derive(Clone, Copy, Debug)]
pub struct CategorizationOptions {
    pub nearest_neighbors: usize,
    pub minimum_amount_gap_ratio: f64,
    pub recency_half_life_days: f64,
    pub minimum_winner_share: f64,
    pub minimum_winner_neighbors: usize,
}

impl Default for CategorizationOptions {
    fn default() -> Self {
        Self {
            nearest_neighbors: 5,
            minimum_amount_gap_ratio: 10.0,
            recency_half_life_days: 90.0,
            minimum_winner_share: 0.65,
            minimum_winner_neighbors: 2,
        }
    }
}

pub struct CategorizationPipeline {
    accounts: HashMap<String, CategoryAccount>,
    history: Vec<CategorizationExample>,
    options: CategorizationOptions,
}

impl CategorizationPipeline {
    pub fn new(accounts: &[CategoryAccount], history: &[CategorizationExample]) -> Self {
        Self::with_options(accounts, history, CategorizationOptions::default())
    }

    pub fn with_options(
        accounts: &[CategoryAccount],
        history: &[CategorizationExample],
        options: CategorizationOptions,
    ) -> Self {
        Self {
            accounts: accounts
                .iter()
                .cloned()
                .map(|account| (account.id.clone(), account))
                .collect(),
            history: history
                .iter()
                .filter(|example| {
                    example.transfer
                        || accounts
                            .iter()
                            .any(|account| account.id == example.counterpart_account_id)
                })
                .cloned()
                .collect(),
            options,
        }
    }

    pub fn resolve(&self, query: &CategorizationQuery) -> CategorizationResolution {
        self.resolve_with_suggestion(query, None)
    }

    pub fn resolve_with_suggestion(
        &self,
        query: &CategorizationQuery,
        suggested_account_id: Option<&str>,
    ) -> CategorizationResolution {
        let exact = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, example)| normalize_payee(&example.payee) == normalize_payee(&query.payee))
            .filter(|(_, example)| !is_future_history(&query.date, &example.date))
            .collect::<Vec<_>>();
        if exact.is_empty() {
            return self.fallback_resolution(
                Vec::new(),
                "No exact resolved-payee category history",
                suggested_account_id,
            );
        }
        if exact
            .iter()
            .max_by(|(left_index, left), (right_index, right)| {
                left.date
                    .cmp(&right.date)
                    .then_with(|| left_index.cmp(right_index))
            })
            .is_some_and(|(_, example)| example.transfer)
        {
            return CategorizationResolution {
                account_id: None,
                account_name: None,
                potential_transfer: true,
                method: CategorizationMethod::LlmFallback,
                reason: "Most recent exact-payee transaction is a transfer".into(),
                agreement: 1.0,
                support: 1.0,
                margin: 1.0,
                candidates: Vec::new(),
                neighbors: Vec::new(),
            };
        }
        let same_direction = exact
            .into_iter()
            .filter(|(_, example)| example.amount.signum() == query.amount.signum())
            .collect::<Vec<_>>();
        if same_direction.is_empty() {
            return self.fallback_resolution(
                Vec::new(),
                "No exact payee history with the same amount direction",
                suggested_account_id,
            );
        }
        let (comparable, amount_regime_detected) = amount_regime(
            query.amount,
            same_direction,
            self.options.minimum_amount_gap_ratio,
        );
        if comparable.is_empty() {
            return self.fallback_resolution(
                Vec::new(),
                "Incoming amount belongs to a new exact-payee amount regime",
                suggested_account_id,
            );
        }
        let distinct_categories = comparable
            .iter()
            .map(|(_, example)| {
                if example.transfer {
                    TRANSFER_OUTCOME_ID
                } else {
                    example.counterpart_account_id.as_str()
                }
            })
            .collect::<HashSet<_>>();
        let mut neighbors = comparable
            .into_iter()
            .map(|(_, example)| self.neighbor(query, example))
            .collect::<Vec<_>>();
        sort_neighbors(&mut neighbors);
        neighbors.truncate(self.options.nearest_neighbors);
        let candidates = aggregate_candidates(&neighbors);
        let winner = candidates.first().expect("exact history has a category");
        let runner_up_share = candidates
            .get(1)
            .map_or(0.0, |candidate| candidate.agreement);
        let share_margin = round(winner.agreement - runner_up_share);
        let confident = distinct_categories.len() == 1
            || (winner.agreement >= self.options.minimum_winner_share
                && winner.neighbor_count >= self.options.minimum_winner_neighbors);
        if !confident {
            return CategorizationResolution {
                account_id: None,
                account_name: None,
                potential_transfer: false,
                method: CategorizationMethod::LlmFallback,
                reason: "Exact payee history remains ambiguous after contextual scoring".into(),
                agreement: winner.agreement,
                support: winner.vote,
                margin: share_margin,
                candidates,
                neighbors,
            };
        }
        if winner.account_id == TRANSFER_OUTCOME_ID {
            return CategorizationResolution {
                account_id: None,
                account_name: None,
                potential_transfer: true,
                method: CategorizationMethod::ExactHistory,
                reason: "Transfer is the winning top-five exact-payee outcome".into(),
                agreement: winner.agreement,
                support: winner.vote,
                margin: share_margin,
                candidates,
                neighbors,
            };
        }
        CategorizationResolution {
            account_id: Some(winner.account_id.clone()),
            account_name: Some(winner.account_name.clone()),
            potential_transfer: false,
            method: CategorizationMethod::ExactHistory,
            reason: if distinct_categories.len() == 1 {
                if amount_regime_detected {
                    "All exact payee history in the matching amount regime uses one category".into()
                } else {
                    "All same-direction exact payee history uses one category".into()
                }
            } else {
                "Top-five comparable exact-payee history cleared ambiguity thresholds".into()
            },
            agreement: winner.agreement,
            support: winner.vote,
            margin: share_margin,
            candidates,
            neighbors,
        }
    }

    fn neighbor(
        &self,
        query: &CategorizationQuery,
        example: &CategorizationExample,
    ) -> CategorizationNeighbor {
        let age_days = date_age_days(&query.date, &example.date).unwrap_or(0);
        let recency = recency_weight(age_days, self.options.recency_half_life_days);
        let similarity = transaction_similarity(query, example, recency);
        let (account_id, account_name) = if example.transfer {
            (TRANSFER_OUTCOME_ID.into(), "Transfer".into())
        } else {
            let account = &self.accounts[&example.counterpart_account_id];
            (account.id.clone(), account.name.clone())
        };
        CategorizationNeighbor {
            payee: example.payee.clone(),
            account_id,
            account_name,
            similarity,
            recency,
            vote: similarity,
        }
    }

    fn fallback_resolution(
        &self,
        neighbors: Vec<CategorizationNeighbor>,
        reason: &str,
        suggested_account_id: Option<&str>,
    ) -> CategorizationResolution {
        let mut resolution = fallback_resolution(neighbors, reason);
        if let Some(account) = suggested_account_id.and_then(|id| self.accounts.get(id)) {
            resolution.account_id = Some(account.id.clone());
            resolution.account_name = Some(account.name.clone());
            resolution.potential_transfer = false;
            resolution.reason = "Local history abstained; using LLM-suggested category".into();
        }
        resolution
    }
}

fn transaction_similarity(
    query: &CategorizationQuery,
    example: &CategorizationExample,
    recency: f64,
) -> f64 {
    let source_account =
        if query.source_account_id.is_empty() || example.source_account_id.is_empty() {
            0.5
        } else {
            f64::from(query.source_account_id == example.source_account_id)
        };
    let source_type = f64::from(query.source_account_type == example.source_account_type);
    0.80 * recency + 0.15 * source_type + 0.05 * source_account
}

fn amount_regime<'a>(
    query_amount: i64,
    examples: Vec<(usize, &'a CategorizationExample)>,
    minimum_gap_ratio: f64,
) -> (Vec<(usize, &'a CategorizationExample)>, bool) {
    let query_amount = query_amount.unsigned_abs();
    if query_amount == 0 {
        return (examples, false);
    }
    let mut amounts = examples
        .iter()
        .map(|(_, example)| example.amount.unsigned_abs())
        .filter(|amount| *amount > 0)
        .chain(std::iter::once(query_amount))
        .collect::<Vec<_>>();
    amounts.sort_unstable();
    amounts.dedup();
    let gap = amounts
        .windows(2)
        .map(|pair| (pair[0], pair[1], pair[1] as f64 / pair[0] as f64))
        .max_by(|left, right| left.2.total_cmp(&right.2));
    let Some((lower, upper, ratio)) = gap else {
        return (examples, false);
    };
    if ratio < minimum_gap_ratio {
        return (examples, false);
    }
    let boundary = ((lower as f64) * (upper as f64)).sqrt();
    let query_is_high = query_amount as f64 > boundary;
    (
        examples
            .into_iter()
            .filter(|(_, example)| {
                (example.amount.unsigned_abs() as f64 > boundary) == query_is_high
            })
            .collect(),
        true,
    )
}

fn sort_neighbors(neighbors: &mut [CategorizationNeighbor]) {
    neighbors.sort_by(|left, right| {
        right
            .vote
            .total_cmp(&left.vote)
            .then_with(|| right.recency.total_cmp(&left.recency))
    });
}

fn aggregate_candidates(neighbors: &[CategorizationNeighbor]) -> Vec<CategoryCandidate> {
    let total_vote = neighbors.iter().map(|neighbor| neighbor.vote).sum::<f64>();
    let mut grouped = BTreeMap::<String, CategoryCandidate>::new();
    for neighbor in neighbors {
        let candidate = grouped
            .entry(neighbor.account_id.clone())
            .or_insert_with(|| CategoryCandidate {
                account_id: neighbor.account_id.clone(),
                account_name: neighbor.account_name.clone(),
                vote: 0.0,
                neighbor_count: 0,
                agreement: 0.0,
                best_similarity: 0.0,
            });
        candidate.vote += neighbor.vote;
        candidate.neighbor_count += 1;
        candidate.best_similarity = candidate.best_similarity.max(neighbor.similarity);
    }
    let mut candidates = grouped.into_values().collect::<Vec<_>>();
    for candidate in &mut candidates {
        candidate.agreement = if total_vote == 0.0 {
            0.0
        } else {
            round(candidate.vote / total_vote)
        };
        candidate.vote = round(candidate.vote);
    }
    candidates.sort_by(|left, right| {
        right
            .vote
            .total_cmp(&left.vote)
            .then_with(|| left.account_name.cmp(&right.account_name))
    });
    candidates
}

fn fallback_resolution(
    neighbors: Vec<CategorizationNeighbor>,
    reason: &str,
) -> CategorizationResolution {
    let candidates = aggregate_candidates(&neighbors);
    let agreement = candidates
        .first()
        .map_or(0.0, |candidate| candidate.agreement);
    let support = candidates.first().map_or(0.0, |candidate| candidate.vote);
    let second_vote = candidates.get(1).map_or(0.0, |candidate| candidate.vote);
    CategorizationResolution {
        account_id: None,
        account_name: None,
        potential_transfer: false,
        method: CategorizationMethod::LlmFallback,
        reason: reason.into(),
        agreement,
        support,
        margin: round(support - second_vote),
        candidates,
        neighbors,
    }
}

fn date_age_days(query: &str, example: &str) -> Option<i64> {
    let query = civil_days(query)?;
    let example = civil_days(example)?;
    (example <= query).then_some(query - example)
}

fn is_future_history(query: &str, example: &str) -> bool {
    matches!((civil_days(query), civil_days(example)), (Some(query), Some(example)) if example > query)
}

fn recency_weight(age_days: i64, half_life_days: f64) -> f64 {
    0.5_f64.powf(age_days as f64 / half_life_days)
}

fn civil_days(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era)
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> CategorizationFixtures {
        serde_json::from_str(include_str!("examples.json")).unwrap()
    }

    #[test]
    fn recorded_run_fixtures_pass() {
        let mut failures = Vec::new();
        for path in run_fixture::fixture_paths(None).unwrap() {
            let fixture = run_fixture::load_fixture(&path).unwrap();
            failures.extend(run_fixture::verify_fixture(&fixture));
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn fixture_evaluations_pass() {
        let fixtures = fixtures();
        let pipeline = CategorizationPipeline::new(&fixtures.accounts, &fixtures.history);
        for evaluation in fixtures.evaluations {
            let result = pipeline.resolve(&evaluation.query);
            assert_eq!(
                result.account_id, evaluation.expected_account_id,
                "{}",
                evaluation.name
            );
        }
    }

    #[test]
    fn unknown_transaction_falls_back_to_llm() {
        let fixtures = fixtures();
        let pipeline = CategorizationPipeline::new(&fixtures.accounts, &fixtures.history);
        let result = pipeline.resolve(&CategorizationQuery {
            payee: "Unknown Merchant".into(),
            imported_category: None,
            date: String::new(),
            amount: -1234,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
        });
        assert_eq!(result.method, CategorizationMethod::LlmFallback);
        assert!(result.account_id.is_none());
    }

    #[test]
    fn valid_llm_suggestion_is_used_only_when_history_abstains() {
        let fixtures = fixtures();
        let pipeline = CategorizationPipeline::new(&fixtures.accounts, &fixtures.history);
        let unknown = CategorizationQuery {
            payee: "Unknown Merchant".into(),
            imported_category: None,
            date: String::new(),
            amount: -1234,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
        };
        let suggested = pipeline.resolve_with_suggestion(&unknown, Some("groceries"));
        assert_eq!(suggested.account_id.as_deref(), Some("groceries"));
        assert_eq!(suggested.method, CategorizationMethod::LlmFallback);

        let exact = CategorizationQuery {
            payee: "Kroger".into(),
            imported_category: Some("groceries".into()),
            date: String::new(),
            amount: -1234,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
        };
        let historical = pipeline.resolve_with_suggestion(&exact, Some("salary"));
        assert_eq!(historical.account_id.as_deref(), Some("groceries"));
        assert_eq!(historical.method, CategorizationMethod::ExactHistory);

        let opposite_direction = CategorizationQuery {
            amount: 1234,
            ..exact.clone()
        };
        let direction_fallback =
            pipeline.resolve_with_suggestion(&opposite_direction, Some("salary"));
        assert_eq!(direction_fallback.account_id.as_deref(), Some("salary"));
        assert_eq!(direction_fallback.method, CategorizationMethod::LlmFallback);

        let conflicting = CategorizationQuery {
            payee: "Target".into(),
            imported_category: None,
            date: String::new(),
            amount: -5000,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
        };
        let unresolved = pipeline.resolve_with_suggestion(&conflicting, Some("salary"));
        assert_eq!(unresolved.account_id, None);
        assert_eq!(unresolved.method, CategorizationMethod::LlmFallback);
        assert_eq!(
            unresolved.reason,
            "Exact payee history remains ambiguous after contextual scoring"
        );
    }

    #[test]
    fn large_amount_gap_selects_the_incoming_amount_regime() {
        let accounts = vec![
            CategoryAccount {
                id: "kids".into(),
                name: "Kids".into(),
                account_type: CategoryAccountType::Expense,
            },
            CategoryAccount {
                id: "bills".into(),
                name: "Bills".into(),
                account_type: CategoryAccountType::Expense,
            },
        ];
        let example = |amount: i64, category: &str| CategorizationExample {
            payee: "Starbucks".into(),
            imported_category: None,
            date: String::new(),
            amount: -amount,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
            counterpart_account_id: category.into(),
            transfer: false,
        };
        let history = vec![
            example(1_000, "kids"),
            example(1_200, "kids"),
            example(1_500, "kids"),
            example(100_000, "bills"),
        ];
        let pipeline = CategorizationPipeline::new(&accounts, &history);
        let query = |amount: i64| CategorizationQuery {
            payee: "Starbucks".into(),
            imported_category: None,
            date: String::new(),
            amount: -amount,
            source_account_type: SourceAccountType::Asset,
            source_account_id: String::new(),
        };

        assert_eq!(
            pipeline.resolve(&query(110_000)).account_id.as_deref(),
            Some("bills")
        );
        assert_eq!(
            pipeline.resolve(&query(1_300)).account_id.as_deref(),
            Some("kids")
        );

        let low_only = CategorizationPipeline::new(&accounts, &history[..3]);
        let novel = low_only.resolve_with_suggestion(&query(110_000), Some("bills"));
        assert_eq!(novel.account_id.as_deref(), Some("bills"));
        assert_eq!(novel.method, CategorizationMethod::LlmFallback);
    }

    #[test]
    fn contextually_indistinguishable_exact_history_remains_ambiguous() {
        let accounts = vec![
            CategoryAccount {
                id: "food".into(),
                name: "Food".into(),
                account_type: CategoryAccountType::Expense,
            },
            CategoryAccount {
                id: "beef".into(),
                name: "Beef".into(),
                account_type: CategoryAccountType::Expense,
            },
        ];
        let history = ["food", "food", "beef", "beef", "beef"]
            .into_iter()
            .map(|account| CategorizationExample {
                payee: "Starbucks".into(),
                imported_category: None,
                date: String::new(),
                amount: -500,
                source_account_type: SourceAccountType::Asset,
                source_account_id: String::new(),
                counterpart_account_id: account.into(),
                transfer: false,
            })
            .collect::<Vec<_>>();
        let result =
            CategorizationPipeline::new(&accounts, &history).resolve(&CategorizationQuery {
                payee: "Starbucks".into(),
                imported_category: None,
                date: String::new(),
                amount: -500,
                source_account_type: SourceAccountType::Asset,
                source_account_id: String::new(),
            });
        assert_eq!(result.method, CategorizationMethod::LlmFallback);
        assert!(result.account_id.is_none());
        assert!(result.agreement < 0.80);
    }

    #[test]
    fn calendar_recency_can_distinguish_multi_category_exact_history() {
        let accounts = vec![
            CategoryAccount {
                id: "food".into(),
                name: "Food".into(),
                account_type: CategoryAccountType::Expense,
            },
            CategoryAccount {
                id: "kids".into(),
                name: "Kids".into(),
                account_type: CategoryAccountType::Expense,
            },
        ];
        let example = |date: &str, category: &str| CategorizationExample {
            payee: "Amazon".into(),
            imported_category: None,
            date: date.into(),
            amount: -5000,
            source_account_type: SourceAccountType::Liability,
            source_account_id: "card".into(),
            counterpart_account_id: category.into(),
            transfer: false,
        };
        let history = vec![
            example("2025-01-01", "food"),
            example("2025-01-02", "food"),
            example("2026-07-20", "kids"),
            example("2026-07-21", "kids"),
        ];
        let result =
            CategorizationPipeline::new(&accounts, &history).resolve(&CategorizationQuery {
                payee: "Amazon".into(),
                imported_category: None,
                date: "2026-08-01".into(),
                amount: -5000,
                source_account_type: SourceAccountType::Liability,
                source_account_id: "card".into(),
            });

        assert_eq!(result.account_id.as_deref(), Some("kids"));
        assert_eq!(date_age_days("2026-08-01", "2026-05-03"), Some(90));
        assert_eq!(recency_weight(90, 90.0), 0.5);
        assert_eq!(recency_weight(180, 90.0), 0.25);
        assert_eq!(recency_weight(270, 90.0), 0.125);
        assert!(is_future_history("2026-08-01", "2026-08-02"));
    }

    #[test]
    fn latest_exact_payee_transfer_bails_immediately() {
        let accounts = vec![CategoryAccount {
            id: "food".into(),
            name: "Food".into(),
            account_type: CategoryAccountType::Expense,
        }];
        let example = |date: &str, transfer: bool| CategorizationExample {
            payee: "Card Payment".into(),
            imported_category: None,
            date: date.into(),
            amount: -5000,
            source_account_type: SourceAccountType::Asset,
            source_account_id: "checking".into(),
            counterpart_account_id: if transfer {
                String::new()
            } else {
                "food".into()
            },
            transfer,
        };
        let history = vec![example("2026-07-01", false), example("2026-07-15", true)];
        let result =
            CategorizationPipeline::new(&accounts, &history).resolve(&CategorizationQuery {
                payee: "Card Payment".into(),
                imported_category: None,
                date: "2026-08-01".into(),
                amount: -5000,
                source_account_type: SourceAccountType::Asset,
                source_account_id: "checking".into(),
            });

        assert!(result.potential_transfer);
        assert_eq!(result.account_id, None);
        assert_eq!(
            result.reason,
            "Most recent exact-payee transaction is a transfer"
        );
    }

    #[test]
    fn transfer_neighbor_only_bails_when_transfer_wins() {
        let accounts = vec![CategoryAccount {
            id: "food".into(),
            name: "Food".into(),
            account_type: CategoryAccountType::Expense,
        }];
        let example = |date: &str, category: Option<&str>| CategorizationExample {
            payee: "Amazon".into(),
            imported_category: None,
            date: date.into(),
            amount: -5000,
            source_account_type: SourceAccountType::Asset,
            source_account_id: "checking".into(),
            counterpart_account_id: category.unwrap_or_default().into(),
            transfer: category.is_none(),
        };
        let query = CategorizationQuery {
            payee: "Amazon".into(),
            imported_category: None,
            date: "2026-08-01".into(),
            amount: -5000,
            source_account_type: SourceAccountType::Asset,
            source_account_id: "checking".into(),
        };
        let category_wins = vec![
            example("2025-01-01", None),
            example("2026-07-01", Some("food")),
            example("2026-07-10", Some("food")),
            example("2026-07-20", Some("food")),
        ];
        let result = CategorizationPipeline::new(&accounts, &category_wins).resolve(&query);
        assert_eq!(result.account_id.as_deref(), Some("food"));
        assert!(!result.potential_transfer);

        let transfer_wins = vec![
            example("2026-07-20", None),
            example("2026-07-21", None),
            example("2026-07-22", Some("food")),
        ];
        let result = CategorizationPipeline::new(&accounts, &transfer_wins).resolve(&query);
        assert!(result.potential_transfer);
        assert_eq!(
            result.reason,
            "Transfer is the winning top-five exact-payee outcome"
        );
    }
}
