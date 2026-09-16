use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const RENDER_VERSION: &str = "payee-prompt-v1";

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

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
