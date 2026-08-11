use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

pub const RENDER_VERSION: &str = "payee-prompt-v1";
pub const SCORE_VERSION: &str = "payee-score-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Payee {
    pub id: String,
    pub raw_payee: String,
    pub direction: String,
    pub potential_payee: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvalInput {
    pub accounts: Vec<Account>,
    pub payees: Vec<Payee>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameters {
    pub include_potentials: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RenderInvocation {
    pub input: EvalInput,
    pub parameters: Parameters,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceRequest {
    pub system: String,
    pub prompt: String,
    pub output_schema: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RenderResult {
    pub version: String,
    pub request: InferenceRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Reference {
    pub payee: String,
    pub category: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelOutput {
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScoreInvocation {
    pub input: EvalInput,
    pub reference: HashMap<String, Reference>,
    pub parameters: Parameters,
    pub output: ModelOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScoreResult {
    pub version: String,
    pub scores: HashMap<String, Option<f64>>,
    pub details: Value,
}

pub fn render(invocation: RenderInvocation) -> RenderResult {
    let mut accounts = invocation.input.accounts;
    accounts.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let parent_ids = accounts
        .iter()
        .filter_map(|account| account.parent_id.as_deref())
        .collect::<HashSet<_>>();
    let by_id = accounts
        .iter()
        .map(|account| (account.id.as_str(), account))
        .collect::<HashMap<_, _>>();
    let categories = accounts
        .iter()
        .filter(|account| {
            matches!(account.account_type.as_str(), "expense" | "income")
                && !(account.parent_id.is_none() && parent_ids.contains(account.id.as_str()))
        })
        .enumerate()
        .map(|(index, account)| {
            let mut names = vec![account.name.as_str()];
            let mut parent_id = account.parent_id.as_deref();
            while let Some(parent) = parent_id.and_then(|id| by_id.get(id).copied()) {
                names.push(parent.name.as_str());
                parent_id = parent.parent_id.as_deref();
            }
            names.reverse();
            (
                format!("c{}", index + 1),
                format!("{}: {}", account.account_type, names.join(" > ")),
            )
        })
        .collect::<Vec<_>>();
    let payees = invocation
        .input
        .payees
        .into_iter()
        .map(|payee| {
            let details = if invocation.parameters.include_potentials {
                payee
                    .potential_payee
                    .map(|potential| {
                        format!(
                            "potential: {}; direction: {}",
                            one_line(&potential),
                            payee.direction
                        )
                    })
                    .unwrap_or_else(|| format!("direction: {}", payee.direction))
            } else {
                format!("direction: {}", payee.direction)
            };
            (payee.id, one_line(&payee.raw_payee), details)
        })
        .collect::<Vec<_>>();
    let properties = payees
        .iter()
        .map(|(id, _, _)| (id.clone(), json!({ "$ref": "#/$defs/result" })))
        .collect::<serde_json::Map<_, _>>();
    let required = payees.iter().map(|(id, _, _)| id).collect::<Vec<_>>();
    let category_values = [Value::Null, json!("transfer")]
        .into_iter()
        .chain(categories.iter().map(|(alias, _)| json!(alias)))
        .collect::<Vec<_>>();
    let output_schema = json!({
        "type": "object",
        "$defs": { "result": {
            "type": "object",
            "properties": {
                "payee": { "type": "string", "minLength": 1 },
                "category": { "type": ["string", "null"], "enum": category_values }
            },
            "required": ["payee", "category"],
            "additionalProperties": false
        }},
        "properties": properties,
        "required": required,
        "additionalProperties": false
    });
    let category_prompt = categories
        .iter()
        .map(|(alias, label)| format!("\n{alias}: {label}"))
        .collect::<String>();
    let payee_prompt = payees
        .iter()
        .map(|(id, payee, details)| format!("{id}: {payee} ({details})"))
        .collect::<Vec<_>>()
        .join("\n");
    RenderResult {
        version: RENDER_VERSION.into(),
        request: InferenceRequest {
            system: "Clean raw imported transaction payees. Each input has a stable ID, the full messy payee, its amount direction, and may have the top local historical potential in parentheses. Inflow means a positive amount and outflow means a negative amount; direction does not by itself imply income or expense because transactions may be transfers, refunds, or payments. The potential may be wrong: reuse it only when it represents the same real payee. Otherwise produce a concise new cleaned payee. Preserve meaningful qualifiers and distinct services or sub-brands. Avoid numbers unless they are an important part of the payee's identity; omit dates, store IDs, transaction references, and other incidental numbers. Return category \"transfer\" when the transaction clearly looks like movement between financial accounts. Otherwise return one category ID from the provided CATEGORIES only when confident; return null when uncertain. Return exactly one result for every input ID, keep every ID unchanged, and return no other IDs.".into(),
            prompt: format!("CATEGORIES\ntransfer: Transfer between financial accounts{category_prompt}\n\nUNRESOLVED PAYEES\n{payee_prompt}"),
            output_schema,
        },
    }
}

pub fn score(invocation: ScoreInvocation) -> Result<ScoreResult, serde_json::Error> {
    let output = parse_output(&invocation.output.text)?;
    let output = output.as_object().cloned().unwrap_or_default();
    let count = invocation.input.payees.len();
    let schema = count == output.len()
        && invocation
            .input
            .payees
            .iter()
            .all(|payee| output.get(&payee.id).is_some_and(Value::is_object));
    let mut payee_correct = 0;
    let mut category_correct = 0;
    let mut good_total = 0;
    let mut good_accepted = 0;
    let mut bad_total = 0;
    let mut bad_rejected = 0;
    for payee in &invocation.input.payees {
        let Some(reference) = invocation.reference.get(&payee.id) else {
            continue;
        };
        let actual = output.get(&payee.id).and_then(Value::as_object);
        let actual_payee = actual
            .and_then(|value| value.get("payee"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if normalize(actual_payee) == normalize(&reference.payee) {
            payee_correct += 1;
        }
        let actual_category = actual.and_then(|value| value.get("category"));
        let category_matches = match (&reference.category, actual_category) {
            (None, Some(Value::Null)) => true,
            (Some(expected), Some(Value::String(actual))) => expected == actual,
            _ => false,
        };
        if category_matches {
            category_correct += 1;
        }
        if invocation.parameters.include_potentials
            && let Some(potential) = payee.potential_payee.as_deref()
        {
            let correct = normalize(potential) == normalize(&reference.payee);
            let accepted = normalize(potential) == normalize(actual_payee);
            if correct {
                good_total += 1;
                if accepted {
                    good_accepted += 1;
                }
            } else {
                bad_total += 1;
                if !accepted {
                    bad_rejected += 1;
                }
            }
        }
    }
    let reference_count = invocation.reference.len();
    let mut scores = HashMap::new();
    scores.insert("schema".into(), Some(if schema { 1.0 } else { 0.0 }));
    scores.insert("payee".into(), ratio(payee_correct, reference_count));
    scores.insert("category".into(), ratio(category_correct, reference_count));
    scores.insert("goodHints".into(), ratio(good_accepted, good_total));
    scores.insert("badHints".into(), ratio(bad_rejected, bad_total));
    Ok(ScoreResult {
        version: SCORE_VERSION.into(),
        scores,
        details: json!({}),
    })
}

fn parse_output(text: &str) -> Result<Value, serde_json::Error> {
    let trimmed = text.trim();
    serde_json::from_str(trimmed).or_else(|error| {
        let inner = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|value| value.strip_suffix("```"));
        inner.map_or(Err(error), |value| serde_json::from_str(value.trim()))
    })
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn normalize(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn ratio(value: usize, total: usize) -> Option<f64> {
    (total > 0).then_some(value as f64 / total as f64)
}
