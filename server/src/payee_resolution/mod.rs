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
pub struct GeneratedPayee {
    pub imported_payee: String,
    pub canonical_payee: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeCandidate {
    pub resolved_payee: String,
    pub score: f64,
    pub character_score: f64,
    pub tfidf_score: f64,
    pub token_score: f64,
    pub neighbor_count: usize,
    pub neighbor_vote: f64,
    pub neighbor_agreement: f64,
    pub aliases: Vec<String>,
    pub examples: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeResolution {
    pub imported_payee: String,
    pub resolved_payee: Option<String>,
    pub method: ResolutionMethod,
    pub score: f64,
    pub margin: f64,
    pub effective_support: f64,
    pub neighbor_agreement: f64,
    pub reason: String,
    pub candidates: Vec<PayeeCandidate>,
    pub neighbors: Vec<PayeeNeighbor>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionMethod {
    Exact,
    Fuzzy,
    Unresolved,
}

#[derive(Clone, Copy, Debug)]
pub struct PayeeResolverOptions {
    pub minimum_score: f64,
    pub minimum_supported_score: f64,
    pub ngram_minimum: usize,
    pub ngram_maximum: usize,
    pub nearest_neighbors: usize,
    pub neighbor_similarity_floor: f64,
    pub recency_half_life: f64,
    pub minimum_neighbors: usize,
    pub minimum_neighbor_agreement: f64,
    pub minimum_effective_support: f64,
    pub minimum_vote_margin: f64,
    pub near_exact_score: f64,
    pub strong_support: f64,
    pub strong_neighbor_count: usize,
}

impl Default for PayeeResolverOptions {
    fn default() -> Self {
        Self {
            minimum_score: 0.68,
            minimum_supported_score: 0.50,
            ngram_minimum: 3,
            ngram_maximum: 5,
            nearest_neighbors: 5,
            neighbor_similarity_floor: 0.30,
            recency_half_life: 500.0,
            minimum_neighbors: 3,
            minimum_neighbor_agreement: 0.85,
            minimum_effective_support: 1.0,
            minimum_vote_margin: 0.35,
            near_exact_score: 0.97,
            strong_support: 1.50,
            strong_neighbor_count: 5,
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

struct Group {
    key: String,
    display_name: String,
    aliases: BTreeMap<String, String>,
    examples: usize,
    ngrams: HashSet<String>,
}

struct HistoricalExample {
    imported_payee: String,
    resolved_key: String,
    position: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeNeighbor {
    pub imported_payee: String,
    pub resolved_payee: String,
    pub similarity: f64,
    pub recency: f64,
    pub vote: f64,
}

pub struct PayeeResolver {
    groups: Vec<Group>,
    idf: HashMap<String, f64>,
    history: Vec<HistoricalExample>,
    options: PayeeResolverOptions,
}

impl PayeeResolver {
    pub fn new(examples: &[PayeeExample]) -> Self {
        Self::with_options(examples, PayeeResolverOptions::default())
    }

    pub fn with_options(examples: &[PayeeExample], options: PayeeResolverOptions) -> Self {
        let mut grouped = BTreeMap::<String, Group>::new();
        let mut history = Vec::with_capacity(examples.len());
        for (position, example) in examples.iter().enumerate() {
            let key = normalize_payee(&example.resolved_payee);
            history.push(HistoricalExample {
                imported_payee: example.imported_payee.clone(),
                resolved_key: key.clone(),
                position,
            });
            let group = grouped.entry(key).or_insert_with(|| Group {
                key: normalize_payee(&example.resolved_payee),
                display_name: example.resolved_payee.clone(),
                aliases: BTreeMap::new(),
                examples: 0,
                ngrams: HashSet::new(),
            });
            group.aliases.insert(
                normalize_payee(&example.imported_payee),
                example.imported_payee.clone(),
            );
            group.examples += 1;
        }
        let mut groups = grouped.into_values().collect::<Vec<_>>();
        for group in &mut groups {
            for alias in group.aliases.values() {
                group.ngrams.extend(character_ngrams(
                    alias,
                    options.ngram_minimum,
                    options.ngram_maximum,
                ));
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
                let weight = (((groups.len() + 1) as f64) / ((count + 1) as f64)).ln() + 1.0;
                (ngram, weight)
            })
            .collect();
        Self {
            groups,
            idf,
            history,
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

    pub fn resolve(&self, query: &PayeeQuery) -> PayeeResolution {
        let normalized = normalize_payee(&query.imported_payee);
        let exact = self
            .groups
            .iter()
            .filter(|group| group.aliases.contains_key(&normalized))
            .collect::<Vec<_>>();
        if exact.len() == 1 {
            return self.exact_result(query, exact[0], ResolutionMethod::Exact);
        }
        if exact.len() > 1 {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: 1.0,
                margin: 0.0,
                effective_support: 0.0,
                neighbor_agreement: 0.0,
                reason: "Exact imported payee has conflicting historical resolutions".into(),
                candidates: exact
                    .into_iter()
                    .map(|group| exact_candidate(group))
                    .collect(),
                neighbors: Vec::new(),
            };
        }

        let query_ngrams = character_ngrams(
            &query.imported_payee,
            self.options.ngram_minimum,
            self.options.ngram_maximum,
        );
        let query_tokens = tokens(&query.imported_payee);
        let group_names = self
            .groups
            .iter()
            .map(|group| (group.key.as_str(), group.display_name.as_str()))
            .collect::<HashMap<_, _>>();
        let mut weighted_neighbors = self
            .history
            .iter()
            .filter_map(|example| {
                let (_, _, _, similarity) =
                    self.alias_scores(&query_ngrams, &query_tokens, &example.imported_payee);
                if similarity < self.options.neighbor_similarity_floor {
                    return None;
                }
                let age = self.history.len().saturating_sub(example.position + 1) as f64;
                let recency = 0.5_f64.powf(age / self.options.recency_half_life);
                let vote = similarity * similarity * recency;
                Some((
                    PayeeNeighbor {
                        imported_payee: example.imported_payee.clone(),
                        resolved_payee: group_names[example.resolved_key.as_str()].into(),
                        similarity: round(similarity),
                        recency: round(recency),
                        vote: round(vote),
                    },
                    example.resolved_key.clone(),
                ))
            })
            .collect::<Vec<_>>();
        weighted_neighbors.sort_by(|left, right| {
            right
                .0
                .similarity
                .total_cmp(&left.0.similarity)
                .then_with(|| right.0.recency.total_cmp(&left.0.recency))
        });
        weighted_neighbors.truncate(self.options.nearest_neighbors);
        let mut votes = HashMap::<String, (f64, usize)>::new();
        for (neighbor, resolved_key) in &weighted_neighbors {
            let entry = votes.entry(resolved_key.clone()).or_default();
            entry.0 += neighbor.vote;
            entry.1 += 1;
        }
        let total_vote = votes.values().map(|(vote, _)| vote).sum::<f64>();
        let neighbors = weighted_neighbors
            .iter()
            .map(|(neighbor, _)| neighbor.clone())
            .collect::<Vec<_>>();
        let mut candidates = self
            .groups
            .iter()
            .map(|group| {
                let mut character_score: f64 = 0.0;
                let mut tfidf_score: f64 = 0.0;
                let mut token_score: f64 = 0.0;
                let mut lexical_score: f64 = 0.0;
                for alias in group.aliases.values() {
                    let (character, tfidf, token, combined) =
                        self.alias_scores(&query_ngrams, &query_tokens, alias);
                    character_score = character_score.max(character);
                    tfidf_score = tfidf_score.max(tfidf);
                    token_score = token_score.max(token);
                    lexical_score = lexical_score.max(combined);
                }
                let (neighbor_vote, neighbor_count) =
                    votes.get(&group.key).copied().unwrap_or_default();
                PayeeCandidate {
                    resolved_payee: group.display_name.clone(),
                    score: round(lexical_score),
                    character_score: round(character_score),
                    tfidf_score: round(tfidf_score),
                    token_score: round(token_score),
                    neighbor_count,
                    neighbor_vote: round(neighbor_vote),
                    neighbor_agreement: if total_vote == 0.0 {
                        0.0
                    } else {
                        round(neighbor_vote / total_vote)
                    },
                    aliases: group.aliases.values().cloned().collect(),
                    examples: group.examples,
                }
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .neighbor_vote
                .total_cmp(&left.neighbor_vote)
                .then_with(|| right.score.total_cmp(&left.score))
                .then_with(|| left.resolved_payee.cmp(&right.resolved_payee))
        });
        candidates.truncate(5);
        let best_score = candidates.first().map_or(0.0, |candidate| candidate.score);
        let effective_support = candidates
            .first()
            .map_or(0.0, |candidate| candidate.neighbor_vote);
        let neighbor_agreement = candidates
            .first()
            .map_or(0.0, |candidate| candidate.neighbor_agreement);
        let second_vote = candidates
            .get(1)
            .map_or(0.0, |candidate| candidate.neighbor_vote);
        let margin = round(effective_support - second_vote);
        let supported_override = candidates.first().is_some_and(|best| {
            best.score >= self.options.minimum_supported_score
                && best.neighbor_count >= self.options.strong_neighbor_count
                && best.neighbor_agreement >= self.options.minimum_neighbor_agreement
                && best.neighbor_vote >= self.options.strong_support
                && margin >= self.options.minimum_vote_margin
        });
        if best_score < self.options.minimum_score && !supported_override {
            let reason = if neighbor_agreement < self.options.minimum_neighbor_agreement {
                format!(
                    "Neighborhood is split: {:.1}% agreement; minimum is {:.1}%",
                    neighbor_agreement * 100.0,
                    self.options.minimum_neighbor_agreement * 100.0
                )
            } else if margin < self.options.minimum_vote_margin {
                format!(
                    "Weighted vote margin {} did not reach minimum {}",
                    margin, self.options.minimum_vote_margin
                )
            } else {
                format!(
                    "Vote leader lexical score {} did not reach minimum {}",
                    best_score, self.options.minimum_score
                )
            };
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: best_score,
                margin,
                effective_support,
                neighbor_agreement,
                reason,
                candidates,
                neighbors,
            };
        }
        let best = &candidates[0];
        let near_exact = best.score >= self.options.near_exact_score;
        if !near_exact && best.neighbor_count < self.options.minimum_neighbors {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: best_score,
                margin,
                effective_support,
                neighbor_agreement,
                reason: format!(
                    "Winner had {} nearby examples; minimum is {}",
                    best.neighbor_count, self.options.minimum_neighbors
                ),
                candidates,
                neighbors,
            };
        }
        if !near_exact && neighbor_agreement < self.options.minimum_neighbor_agreement {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: best_score,
                margin,
                effective_support,
                neighbor_agreement,
                reason: format!(
                    "Neighbor agreement did not reach minimum {}",
                    self.options.minimum_neighbor_agreement
                ),
                candidates,
                neighbors,
            };
        }
        if !near_exact && effective_support < self.options.minimum_effective_support {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: best_score,
                margin,
                effective_support,
                neighbor_agreement,
                reason: format!(
                    "Effective support did not reach minimum {}",
                    self.options.minimum_effective_support
                ),
                candidates,
                neighbors,
            };
        }
        if !near_exact && margin < self.options.minimum_vote_margin {
            return PayeeResolution {
                imported_payee: query.imported_payee.clone(),
                resolved_payee: None,
                method: ResolutionMethod::Unresolved,
                score: best_score,
                margin,
                effective_support,
                neighbor_agreement,
                reason: format!(
                    "Weighted vote margin did not reach minimum {}",
                    self.options.minimum_vote_margin
                ),
                candidates,
                neighbors,
            };
        }
        PayeeResolution {
            imported_payee: query.imported_payee.clone(),
            resolved_payee: Some(best.resolved_payee.clone()),
            method: ResolutionMethod::Fuzzy,
            score: best.score,
            margin,
            effective_support,
            neighbor_agreement,
            reason: if near_exact {
                "Near-exact historical alias match".into()
            } else if supported_override {
                "Dense, unanimous recent-neighbor support".into()
            } else {
                "High-confidence recency-weighted nearest-neighbor agreement".into()
            },
            candidates,
            neighbors,
        }
    }

    fn alias_scores(
        &self,
        query_ngrams: &HashSet<String>,
        query_tokens: &HashSet<String>,
        alias: &str,
    ) -> (f64, f64, f64, f64) {
        let alias_ngrams = character_ngrams(
            alias,
            self.options.ngram_minimum,
            self.options.ngram_maximum,
        );
        let character = set_cosine(query_ngrams, &alias_ngrams);
        let tfidf = weighted_cosine(query_ngrams, &alias_ngrams, &self.idf);
        let token = jaccard(query_tokens, &tokens(alias));
        let combined = (0.45 * character + 0.35 * tfidf + 0.15 * token) / 0.95;
        (character, tfidf, token, combined)
    }

    fn exact_result(
        &self,
        query: &PayeeQuery,
        group: &Group,
        method: ResolutionMethod,
    ) -> PayeeResolution {
        PayeeResolution {
            imported_payee: query.imported_payee.clone(),
            resolved_payee: Some(group.display_name.clone()),
            method,
            score: 1.0,
            margin: 1.0,
            effective_support: group.examples as f64,
            neighbor_agreement: 1.0,
            reason: "Exact imported payee has one historical resolution".into(),
            candidates: vec![exact_candidate(group)],
            neighbors: Vec::new(),
        }
    }
}

pub fn generate_fuzzy_payees(payee: &str, seed: u64, count: usize) -> Vec<GeneratedPayee> {
    const CITIES: &[&str] = &["RICHMOND VA", "MIDLOTHIAN VA", "ONLINE", "WEB"];
    let mut random = Lcg(seed);
    let mut generated = Vec::with_capacity(count);
    for index in 0..count {
        let uppercase = payee.to_uppercase();
        let compact = uppercase.replace([' ', '\'', '.'], "");
        let number = 1000 + random.next_usize(9000);
        let city = CITIES[random.next_usize(CITIES.len())];
        let imported = match index % 6 {
            0 => format!("POS PURCHASE {uppercase} #{number}"),
            1 => format!("TST*{uppercase} {number}"),
            2 => format!("SQ *{uppercase} #{number} {city}"),
            3 => format!("{compact}*{number}"),
            4 => format!("{uppercase} STORE {number} {city}"),
            _ => format!("DEBIT CARD {uppercase} {number}"),
        };
        generated.push(GeneratedPayee {
            imported_payee: imported,
            canonical_payee: payee.into(),
        });
    }
    generated
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

fn normalize_for_similarity(value: &str) -> String {
    let normalized = normalize_payee(value);
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

fn exact_candidate(group: &Group) -> PayeeCandidate {
    PayeeCandidate {
        resolved_payee: group.display_name.clone(),
        score: 1.0,
        character_score: 1.0,
        tfidf_score: 1.0,
        token_score: 1.0,
        neighbor_count: group.examples,
        neighbor_vote: group.examples as f64,
        neighbor_agreement: 1.0,
        aliases: group.aliases.values().cloned().collect(),
        examples: group.examples,
    }
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
    fn conflicting_exact_payees_remain_unresolved() {
        let fixtures = fixtures();
        let resolver = PayeeResolver::new(&fixtures.history);
        assert!(
            resolver
                .resolve(&PayeeQuery {
                    imported_payee: "PAYMENT".into(),
                })
                .resolved_payee
                .is_none()
        );
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
    fn multiple_nearby_examples_supply_effective_support() {
        let generated = generate_fuzzy_payees("Starbucks", 42, 4);
        let history = generated[..3]
            .iter()
            .map(|payee| PayeeExample {
                imported_payee: payee.imported_payee.clone(),
                resolved_payee: "Starbucks".into(),
            })
            .collect::<Vec<_>>();
        let result = PayeeResolver::new(&history).resolve(&PayeeQuery {
            imported_payee: generated[3].imported_payee.clone(),
        });
        assert_eq!(result.resolved_payee.as_deref(), Some("Starbucks"));
        assert_eq!(result.candidates[0].neighbor_count, 3);
        assert_eq!(result.neighbor_agreement, 1.0);
        assert!(result.effective_support >= 1.0);
    }
}
