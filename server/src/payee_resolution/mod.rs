use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeExample {
    pub imported_payee: String,
    pub resolved_payee: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeQuery {
    pub imported_payee: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeEvaluation {
    pub name: String,
    pub imported_payee: String,
    pub expected: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PayeeFixtures {
    pub history: Vec<PayeeExample>,
    pub evaluations: Vec<PayeeEvaluation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeCandidate {
    pub imported_payee: String,
    pub resolved_payee: String,
    pub comparison_form: String,
    pub score: f64,
    pub position: usize,
    pub eligible: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeResolution {
    pub imported_payee: String,
    pub resolved_payee: Option<String>,
    pub potential_payee: Option<String>,
    pub method: ResolutionMethod,
    pub best_score: f64,
    pub runner_up_score: Option<f64>,
    pub minimum_score: f64,
    pub minimum_group_margin: f64,
    pub reason: String,
    pub selected_position: Option<usize>,
    pub candidates: Vec<PayeeCandidate>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionMethod {
    ExactLatest,
    FuzzyBest,
    Unresolved,
}

#[derive(Clone, Copy, Debug)]
pub struct PayeeResolverOptions {
    pub minimum_score: f64,
    pub minimum_supported_score: f64,
    pub minimum_group_margin: f64,
    pub ngram_minimum: usize,
    pub ngram_maximum: usize,
}

impl Default for PayeeResolverOptions {
    fn default() -> Self {
        Self {
            minimum_score: 0.90,
            minimum_supported_score: 0.50,
            minimum_group_margin: 0.05,
            ngram_minimum: 3,
            ngram_maximum: 5,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeGroup {
    pub resolved_payee: String,
    pub aliases: Vec<String>,
    pub examples: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeIndexStats {
    pub occurrences: usize,
    pub exact_keys: usize,
    pub comparison_forms: usize,
    pub indexed_ngrams: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedPayee {
    pub imported_payee: String,
    pub canonical_payee: String,
}

struct Occurrence {
    imported_payee: String,
    resolved_payee: String,
    position: usize,
}

struct IndexedForm {
    comparison_form: String,
    ngrams: HashSet<String>,
    occurrences: Vec<usize>,
}

struct Group {
    key: String,
    display_name: String,
    aliases: BTreeMap<String, String>,
    examples: usize,
    ngrams: HashSet<String>,
    stable_ngrams: HashSet<String>,
    comparison_forms: usize,
}

pub struct PayeeResolver {
    occurrences: Vec<Occurrence>,
    exact_index: HashMap<String, Vec<usize>>,
    forms: Vec<IndexedForm>,
    ngram_index: HashMap<String, Vec<usize>>,
    groups: Vec<Group>,
    idf: HashMap<String, f64>,
    options: PayeeResolverOptions,
}

impl PayeeResolver {
    pub fn new(examples: &[PayeeExample]) -> Self {
        Self::with_options(examples, PayeeResolverOptions::default())
    }

    pub fn with_options(examples: &[PayeeExample], options: PayeeResolverOptions) -> Self {
        let occurrences = examples
            .iter()
            .enumerate()
            .map(|(position, example)| Occurrence {
                imported_payee: example.imported_payee.clone(),
                resolved_payee: example.resolved_payee.clone(),
                position,
            })
            .collect::<Vec<_>>();
        let mut exact_index = HashMap::<String, Vec<usize>>::new();
        let mut forms = BTreeMap::<String, Vec<usize>>::new();
        let mut grouped = BTreeMap::<String, Group>::new();
        for (index, example) in examples.iter().enumerate() {
            exact_index
                .entry(normalize_payee(&example.imported_payee))
                .or_default()
                .push(index);
            forms
                .entry(normalize_for_similarity(&example.imported_payee))
                .or_default()
                .push(index);
            let group = grouped
                .entry(normalize_payee(&example.resolved_payee))
                .or_insert_with(|| Group {
                    key: normalize_payee(&example.resolved_payee),
                    display_name: example.resolved_payee.clone(),
                    aliases: BTreeMap::new(),
                    examples: 0,
                    ngrams: HashSet::new(),
                    stable_ngrams: HashSet::new(),
                    comparison_forms: 0,
                });
            group.aliases.insert(
                normalize_payee(&example.imported_payee),
                example.imported_payee.clone(),
            );
            group.examples += 1;
        }
        let mut groups = grouped.into_values().collect::<Vec<_>>();
        for group in &mut groups {
            let comparison_forms = group
                .aliases
                .values()
                .map(|alias| normalize_for_similarity(alias))
                .collect::<HashSet<_>>();
            group.comparison_forms = comparison_forms.len();
            for (index, comparison_form) in comparison_forms.iter().enumerate() {
                let ngrams = character_ngrams(
                    comparison_form,
                    options.ngram_minimum,
                    options.ngram_maximum,
                );
                group.ngrams.extend(ngrams.iter().cloned());
                if index == 0 {
                    group.stable_ngrams = ngrams;
                } else {
                    group.stable_ngrams.retain(|ngram| ngrams.contains(ngram));
                }
            }
        }
        let mut document_frequency = HashMap::<String, usize>::new();
        for group in &groups {
            for ngram in &group.ngrams {
                *document_frequency.entry(ngram.clone()).or_default() += 1;
            }
        }
        let idf = document_frequency
            .into_iter()
            .map(|(ngram, count)| {
                (
                    ngram,
                    (((groups.len() + 1) as f64) / ((count + 1) as f64)).ln() + 1.0,
                )
            })
            .collect::<HashMap<_, _>>();
        let forms = forms
            .into_iter()
            .map(|(comparison_form, occurrences)| IndexedForm {
                ngrams: character_ngrams(
                    &comparison_form,
                    options.ngram_minimum,
                    options.ngram_maximum,
                ),
                comparison_form,
                occurrences,
            })
            .collect::<Vec<_>>();
        let mut ngram_index = HashMap::<String, Vec<usize>>::new();
        for (form_id, form) in forms.iter().enumerate() {
            for ngram in &form.ngrams {
                ngram_index.entry(ngram.clone()).or_default().push(form_id);
            }
        }
        Self {
            occurrences,
            exact_index,
            forms,
            ngram_index,
            groups,
            idf,
            options,
        }
    }

    pub fn groups(&self) -> Vec<PayeeGroup> {
        self.groups
            .iter()
            .map(|group| PayeeGroup {
                resolved_payee: group.display_name.clone(),
                aliases: group.aliases.values().cloned().collect(),
                examples: group.examples,
            })
            .collect()
    }

    pub fn index_stats(&self) -> PayeeIndexStats {
        PayeeIndexStats {
            occurrences: self.occurrences.len(),
            exact_keys: self.exact_index.len(),
            comparison_forms: self.forms.len(),
            indexed_ngrams: self.ngram_index.len(),
        }
    }

    pub fn resolve(&self, query: &PayeeQuery) -> PayeeResolution {
        let exact_key = normalize_payee(&query.imported_payee);
        if let Some(indices) = self.exact_index.get(&exact_key) {
            let selected = indices
                .iter()
                .map(|index| &self.occurrences[*index])
                .max_by_key(|occurrence| occurrence.position)
                .expect("exact index is not empty");
            let mut selected_candidate = candidate(selected, &exact_key, 1.0, true);
            selected_candidate.selected = true;
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: Some(selected.resolved_payee.clone()),
                potential_payee: Some(selected.resolved_payee.clone()),
                method: ResolutionMethod::ExactLatest,
                best_score: 1.0,
                runner_up_score: None,
                minimum_score: self.options.minimum_score,
                minimum_group_margin: self.options.minimum_group_margin,
                reason: "Most recent exact normalized imported-payee match".into(),
                selected_position: Some(selected.position),
                candidates: vec![selected_candidate],
            };
        }

        let comparison_form = normalize_for_similarity(&query.imported_payee);
        let query_ngrams = character_ngrams(
            &comparison_form,
            self.options.ngram_minimum,
            self.options.ngram_maximum,
        );
        let mut form_ids = HashSet::new();
        for ngram in &query_ngrams {
            if let Some(ids) = self.ngram_index.get(ngram) {
                form_ids.extend(ids.iter().copied());
            }
        }
        let mut scored_forms = form_ids
            .into_iter()
            .map(|form_id| {
                let form = &self.forms[form_id];
                let score = round(weighted_cosine(&query_ngrams, &form.ngrams, &self.idf));
                (form_id, score)
            })
            .collect::<Vec<_>>();
        scored_forms.sort_by(|left, right| right.1.total_cmp(&left.1));
        let mut group_scores = HashMap::<String, f64>::new();
        for (form_id, score) in &scored_forms {
            for index in &self.forms[*form_id].occurrences {
                let group = normalize_payee(&self.occurrences[*index].resolved_payee);
                group_scores
                    .entry(group)
                    .and_modify(|best| *best = best.max(*score))
                    .or_insert(*score);
            }
        }
        let mut ranked_groups = self
            .groups
            .iter()
            .filter_map(|group| {
                let alias_score = group_scores.get(&group.key).copied()?;
                let supported = group.comparison_forms >= 2 && !group.stable_ngrams.is_empty();
                let score = if supported {
                    round(weighted_cosine(
                        &query_ngrams,
                        &group.stable_ngrams,
                        &self.idf,
                    ))
                } else {
                    alias_score
                };
                Some((group.key.clone(), score, supported))
            })
            .collect::<Vec<_>>();
        ranked_groups.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        let best_score = ranked_groups.first().map_or(0.0, |group| group.1);
        let runner_up_score = ranked_groups.get(1).map(|group| group.1);
        let minimum_score = ranked_groups
            .first()
            .map_or(self.options.minimum_score, |group| {
                if group.2 {
                    self.options.minimum_supported_score
                } else {
                    self.options.minimum_score
                }
            });
        let potential_payee = (best_score >= minimum_score)
            .then(|| {
                ranked_groups.first().and_then(|group| {
                    self.groups
                        .iter()
                        .find(|candidate| candidate.key == group.0)
                        .map(|candidate| candidate.display_name.clone())
                })
            })
            .flatten();
        if best_score < minimum_score {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                potential_payee,
                method: ResolutionMethod::Unresolved,
                best_score,
                runner_up_score,
                minimum_score,
                minimum_group_margin: self.options.minimum_group_margin,
                reason: format!(
                    "Best payee-group score {best_score} did not reach minimum {}",
                    minimum_score
                ),
                selected_position: None,
                candidates: self.display_candidates(&scored_forms, None, None, None),
            };
        }
        if let Some(runner_up_score) = runner_up_score
            && round(best_score - runner_up_score) < self.options.minimum_group_margin
        {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                potential_payee,
                method: ResolutionMethod::Unresolved,
                best_score,
                runner_up_score: Some(runner_up_score),
                minimum_score,
                minimum_group_margin: self.options.minimum_group_margin,
                reason: format!(
                    "Best payee group led the runner-up by less than {}",
                    self.options.minimum_group_margin
                ),
                selected_position: None,
                candidates: self.display_candidates(&scored_forms, None, None, None),
            };
        }
        let winning_group = &ranked_groups[0].0;
        let used_stable_profile = ranked_groups[0].2;
        let winning_alias_score = scored_forms
            .iter()
            .filter(|scored| {
                self.forms[scored.0].occurrences.iter().any(|index| {
                    normalize_payee(&self.occurrences[*index].resolved_payee) == *winning_group
                })
            })
            .map(|scored| scored.1)
            .max_by(f64::total_cmp)
            .expect("winning fuzzy group contains a scored alias");
        let selected_index = scored_forms
            .iter()
            .filter(|scored| scored.1 == winning_alias_score)
            .flat_map(|scored| self.forms[scored.0].occurrences.iter())
            .filter(|index| {
                normalize_payee(&self.occurrences[**index].resolved_payee) == *winning_group
            })
            .copied()
            .max_by_key(|index| self.occurrences[*index].position)
            .expect("winning fuzzy group contains a best-scoring occurrence");
        let selected = &self.occurrences[selected_index];
        PayeeResolution {
            imported_payee: query.imported_payee.clone(),
            resolved_payee: Some(selected.resolved_payee.clone()),
            potential_payee: Some(selected.resolved_payee.clone()),
            method: ResolutionMethod::FuzzyBest,
            best_score,
            runner_up_score,
            minimum_score,
            minimum_group_margin: self.options.minimum_group_margin,
            reason: if used_stable_profile {
                "Stable n-gram profile from repeated raw-payee formats won clearly".into()
            } else {
                "Best historical raw-payee match from the clear winning payee group".into()
            },
            selected_position: Some(selected.position),
            candidates: self.display_candidates(
                &scored_forms,
                Some(winning_group),
                Some(winning_alias_score),
                Some(selected_index),
            ),
        }
    }

    fn display_candidates(
        &self,
        scored_forms: &[(usize, f64)],
        eligible_group: Option<&str>,
        eligible_score: Option<f64>,
        selected_index: Option<usize>,
    ) -> Vec<PayeeCandidate> {
        let mut candidates = scored_forms
            .iter()
            .flat_map(|(form_id, score)| {
                let form = &self.forms[*form_id];
                form.occurrences.iter().map(move |index| {
                    let occurrence = &self.occurrences[*index];
                    candidate(
                        occurrence,
                        &form.comparison_form,
                        *score,
                        eligible_group.is_some_and(|group| {
                            normalize_payee(&occurrence.resolved_payee) == group
                        }) && eligible_score == Some(*score),
                    )
                })
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.position.cmp(&left.position))
        });
        candidates.truncate(10);
        if let Some(selected_index) = selected_index {
            for candidate in &mut candidates {
                candidate.selected = candidate.position == selected_index;
            }
        }
        candidates
    }
}

fn candidate(
    occurrence: &Occurrence,
    comparison_form: &str,
    score: f64,
    eligible: bool,
) -> PayeeCandidate {
    PayeeCandidate {
        imported_payee: occurrence.imported_payee.clone(),
        resolved_payee: occurrence.resolved_payee.clone(),
        comparison_form: comparison_form.into(),
        score: round(score),
        position: occurrence.position,
        eligible,
        selected: false,
    }
}

pub fn generate_fuzzy_payees(payee: &str, seed: u64, count: usize) -> Vec<GeneratedPayee> {
    let mut random = Lcg(seed);
    let mut generated = Vec::with_capacity(count);
    for index in 0..count {
        generated.push(GeneratedPayee {
            imported_payee: fuzzy_payee(payee, index, &mut random),
            canonical_payee: payee.into(),
        });
    }
    generated
}

pub fn generate_related_fuzzy_payees(payee: &str, seed: u64, count: usize) -> Vec<GeneratedPayee> {
    const RELATED_SUFFIXES: &[&str] = &["", "Prime", "Video"];
    let mut random = Lcg(seed);
    let mut generated = Vec::with_capacity(count);
    for index in 0..count {
        let suffix = RELATED_SUFFIXES[index % RELATED_SUFFIXES.len()];
        let canonical = if suffix.is_empty() {
            payee.to_owned()
        } else {
            format!("{payee} {suffix}")
        };
        let template = (index / RELATED_SUFFIXES.len()) % 6;
        generated.push(GeneratedPayee {
            imported_payee: fuzzy_payee(&canonical, template, &mut random),
            canonical_payee: canonical,
        });
    }
    generated
}

fn fuzzy_payee(payee: &str, template: usize, random: &mut Lcg) -> String {
    const CITIES: &[&str] = &["RICHMOND VA", "MIDLOTHIAN VA", "ONLINE", "WEB"];
    let uppercase = payee.to_uppercase();
    let compact = uppercase.replace([' ', '\'', '.'], "");
    let number = 1000 + random.next_usize(9000);
    let city = CITIES[random.next_usize(CITIES.len())];
    match template % 6 {
        0 => format!("POS PURCHASE {uppercase} #{number}"),
        1 => format!("TST*{uppercase} {number}"),
        2 => format!("SQ *{uppercase} #{number} {city}"),
        3 => format!("{compact}*{number}"),
        4 => format!("{uppercase} STORE {number} {city}"),
        _ => format!("DEBIT CARD {uppercase} {number}"),
    }
}

struct Lcg(u64);

impl Lcg {
    fn next_usize(&mut self, maximum: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((self.0 >> 32) as usize) % maximum
    }
}

pub fn normalize_payee(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .filter(|character| !character.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn clean_payee_name(value: &str) -> String {
    strip_square_prefix(value)
        .split_whitespace()
        .map(|word| {
            let mut capitalize = true;
            word.chars()
                .flat_map(|character| {
                    if capitalize && character.is_alphabetic() {
                        capitalize = false;
                        character.to_uppercase().collect::<Vec<_>>()
                    } else {
                        character.to_lowercase().collect::<Vec<_>>()
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn payee_similarity(left: &str, right: &str) -> f64 {
    let left_ngrams = character_ngrams(left, 3, 5);
    let right_ngrams = character_ngrams(right, 3, 5);
    let character = set_cosine(&left_ngrams, &right_ngrams);
    let token = jaccard(&tokens(left), &tokens(right));
    round(0.8 * character + 0.2 * token)
}

fn normalize_for_similarity(value: &str) -> String {
    let normalized = normalize_payee(strip_square_prefix(value));
    let mut output = String::new();
    let mut digits = String::new();
    let flush_digits = |output: &mut String, digits: &mut String| {
        if digits.len() >= 3 {
            output.push_str(" number ");
        } else {
            output.push_str(digits);
        }
        digits.clear();
    };
    for character in normalized.chars() {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        flush_digits(&mut output, &mut digits);
        output.push(if character.is_alphanumeric() {
            character
        } else {
            ' '
        });
    }
    flush_digits(&mut output, &mut digits);
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_square_prefix(value: &str) -> &str {
    let value = value.trim();
    let Some(prefix) = value.get(..2) else {
        return value;
    };
    if !prefix.eq_ignore_ascii_case("sq") {
        return value;
    }
    let remainder = value[2..].trim_start();
    remainder
        .strip_prefix('*')
        .map(str::trim_start)
        .unwrap_or(value)
}

fn character_ngrams(value: &str, minimum: usize, maximum: usize) -> HashSet<String> {
    let characters = format!("^{}$", normalize_for_similarity(value))
        .chars()
        .collect::<Vec<_>>();
    let mut result = HashSet::new();
    for size in minimum..=maximum {
        for window in characters.windows(size) {
            result.insert(window.iter().collect());
        }
    }
    result
}

fn tokens(value: &str) -> HashSet<String> {
    normalize_for_similarity(value)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn set_cosine(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count() as f64;
    intersection / ((left.len() * right.len()) as f64).sqrt()
}

fn weighted_cosine(
    left: &HashSet<String>,
    right: &HashSet<String>,
    weights: &HashMap<String, f64>,
) -> f64 {
    let mut dot = 0.0;
    let mut left_length = 0.0;
    let mut right_length = 0.0;
    for item in left {
        let weight = weights.get(item).copied().unwrap_or(1.0);
        left_length += weight * weight;
        if right.contains(item) {
            dot += weight * weight;
        }
    }
    for item in right {
        let weight = weights.get(item).copied().unwrap_or(1.0);
        right_length += weight * weight;
    }
    if left_length == 0.0 || right_length == 0.0 {
        0.0
    } else {
        dot / (left_length * right_length).sqrt()
    }
}

fn jaccard(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count();
    let union = left.len() + right.len() - intersection;
    intersection as f64 / union as f64
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PayeeFixtures {
        serde_json::from_str(include_str!("examples.json")).unwrap()
    }

    #[test]
    fn normalizes_harmless_textual_differences() {
        assert_eq!(
            normalize_payee("  STARBUCKS\t MOBILE  "),
            "starbucks mobile"
        );
    }

    #[test]
    fn cleans_square_prefix_and_title_cases_fallback_payees() {
        assert_eq!(
            clean_payee_name("  SQ *BLANCHARD'S BROAD  "),
            "Blanchard's Broad"
        );
        assert_eq!(
            normalize_for_similarity("SQ *AMAZON MARKETPLACE #1234"),
            normalize_for_similarity("Amazon Marketplace #9876")
        );
    }

    #[test]
    fn passes_fixture_evaluations() {
        let fixtures = fixtures();
        let resolver = PayeeResolver::new(&fixtures.history);
        for evaluation in fixtures.evaluations {
            let result = resolver.resolve(&PayeeQuery {
                imported_payee: evaluation.imported_payee,
            });
            assert_eq!(
                result.resolved_payee, evaluation.expected,
                "{}",
                evaluation.name
            );
        }
    }

    #[test]
    fn repeated_transactions_are_one_idf_document() {
        let fixtures = fixtures();
        let resolver = PayeeResolver::new(&fixtures.history);
        let starbucks = resolver
            .groups()
            .into_iter()
            .find(|group| group.resolved_payee == "Starbucks")
            .unwrap();
        assert_eq!(starbucks.examples, 8);
        assert_eq!(starbucks.aliases.len(), 4);
    }

    #[test]
    fn latest_exact_match_wins() {
        let resolver = PayeeResolver::new(&[
            PayeeExample {
                imported_payee: "STARBUCKS MOBILE".into(),
                resolved_payee: "Starbucks".into(),
            },
            PayeeExample {
                imported_payee: "STARBUCKS MOBILE".into(),
                resolved_payee: "Starbucks Coffee".into(),
            },
        ]);
        let result = resolver.resolve(&PayeeQuery {
            imported_payee: " starbucks   mobile ".into(),
        });
        assert_eq!(result.resolved_payee.as_deref(), Some("Starbucks Coffee"));
        assert_eq!(result.method, ResolutionMethod::ExactLatest);
    }

    #[test]
    fn conflicting_fuzzy_groups_remain_unresolved() {
        let resolver = PayeeResolver::new(&[
            PayeeExample {
                imported_payee: "TST*STARBUCKS 1001".into(),
                resolved_payee: "Starbucks".into(),
            },
            PayeeExample {
                imported_payee: "TST*STARBUCKS 2002".into(),
                resolved_payee: "Starbucks Coffee".into(),
            },
        ]);
        let result = resolver.resolve(&PayeeQuery {
            imported_payee: "TST STARBUCKS 9999".into(),
        });
        assert_eq!(result.resolved_payee, None);
        assert_eq!(result.method, ResolutionMethod::Unresolved);
        assert_eq!(result.runner_up_score, Some(1.0));
        assert_eq!(result.potential_payee.as_deref(), Some("Starbucks"));
    }

    #[test]
    fn recent_weak_match_cannot_override_better_older_match() {
        let resolver = PayeeResolver::new(&[
            PayeeExample {
                imported_payee: "SQ *JOES COFFEE #104 RICHMOND".into(),
                resolved_payee: "Joe's Coffee".into(),
            },
            PayeeExample {
                imported_payee: "RIVER CITY BOOKS".into(),
                resolved_payee: "River City Books".into(),
            },
        ]);
        let result = resolver.resolve(&PayeeQuery {
            imported_payee: "SQ JOES COFFEE #999 RICHMOND".into(),
        });
        assert_eq!(result.resolved_payee.as_deref(), Some("Joe's Coffee"));
    }

    #[test]
    fn amazon_prime_does_not_inherit_plain_amazon() {
        let history = (0..100)
            .map(|number| PayeeExample {
                imported_payee: format!("POS PURCHASE AMAZON #{number:04}"),
                resolved_payee: "Amazon".into(),
            })
            .collect::<Vec<_>>();
        let result = PayeeResolver::new(&history).resolve(&PayeeQuery {
            imported_payee: "POS PURCHASE AMAZON PRIME #4595".into(),
        });

        assert_eq!(result.resolved_payee, None);
        assert_eq!(result.potential_payee, None);
        assert!(result.best_score < result.minimum_score);
    }

    #[test]
    fn plain_amazon_does_not_inherit_amazon_prime() {
        let history = (0..10)
            .map(|number| PayeeExample {
                imported_payee: format!("POS PURCHASE AMAZON PRIME #{number:04}"),
                resolved_payee: "Amazon Prime".into(),
            })
            .collect::<Vec<_>>();
        let result = PayeeResolver::new(&history).resolve(&PayeeQuery {
            imported_payee: "POS PURCHASE AMAZON #4595".into(),
        });

        assert_eq!(result.resolved_payee, None);
        assert!(result.best_score < result.minimum_score);
    }

    #[test]
    fn amazon_prime_matches_after_one_similar_example() {
        let result = PayeeResolver::new(&[PayeeExample {
            imported_payee: "POS PURCHASE AMAZON PRIME #1001".into(),
            resolved_payee: "Amazon Prime".into(),
        }])
        .resolve(&PayeeQuery {
            imported_payee: "POS PURCHASE AMAZON PRIME #4595".into(),
        });

        assert_eq!(result.resolved_payee.as_deref(), Some("Amazon Prime"));
        assert_eq!(result.method, ResolutionMethod::FuzzyBest);
        assert_eq!(result.best_score, 1.0);
    }

    #[test]
    fn related_payees_resolve_after_two_distinct_raw_formats() {
        let generated = generate_related_fuzzy_payees("amazon", 42, 10);
        let mut history = Vec::new();

        for (index, generated) in generated.into_iter().enumerate() {
            let result = PayeeResolver::new(&history).resolve(&PayeeQuery {
                imported_payee: generated.imported_payee.clone(),
            });
            if index < 6 {
                assert_eq!(result.resolved_payee, None, "step {}", index + 1);
            } else {
                assert_eq!(
                    result.resolved_payee.as_deref().map(normalize_payee),
                    Some(normalize_payee(&generated.canonical_payee)),
                    "step {}",
                    index + 1
                );
                assert_eq!(result.minimum_score, 0.5, "step {}", index + 1);
            }
            history.push(PayeeExample {
                imported_payee: generated.imported_payee,
                resolved_payee: normalize_payee(&generated.canonical_payee),
            });
        }
    }

    #[test]
    fn fuzzy_generator_is_reproducible_and_repeats_payees() {
        let first = generate_fuzzy_payees("Starbucks", 42, 12);
        let second = generate_fuzzy_payees("Starbucks", 42, 12);
        assert_eq!(first[0].imported_payee, second[0].imported_payee);
        assert!(
            first
                .iter()
                .all(|payee| payee.canonical_payee == "Starbucks")
        );
        assert_ne!(first[0].imported_payee, first[6].imported_payee);
    }

    #[test]
    fn related_generator_creates_distinct_payee_family() {
        let generated = generate_related_fuzzy_payees("Amazon", 42, 3);
        let names = generated
            .iter()
            .map(|payee| payee.canonical_payee.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["Amazon", "Amazon Prime", "Amazon Video",]);
        assert!(
            generated
                .iter()
                .all(|payee| payee.imported_payee.starts_with("POS PURCHASE"))
        );
    }

    #[test]
    fn index_collapses_fuzzy_forms_but_keeps_occurrences() {
        let resolver = PayeeResolver::new(&[
            PayeeExample {
                imported_payee: "DEBIT CARD STARBUCKS 1001".into(),
                resolved_payee: "Starbucks".into(),
            },
            PayeeExample {
                imported_payee: "DEBIT CARD STARBUCKS 2002".into(),
                resolved_payee: "Starbucks Coffee".into(),
            },
        ]);
        let stats = resolver.index_stats();
        assert_eq!(stats.occurrences, 2);
        assert_eq!(stats.comparison_forms, 1);
    }
}
