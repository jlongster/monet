use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const RENDER_VERSION: &str = "payee-prompt-v2";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub raw_payee: String,
    pub direction: String,
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

pub fn render(input: Input) -> RenderResult {
    RenderResult {
        version: RENDER_VERSION.into(),
        request: InferenceRequest {
            system: "Extract the best concise payee description from a raw financial transaction payee. If it references a store or financial account, extract the store or account name. Remove transaction metadata such as dates, locations, store numbers, card digits, and reference numbers. Return only the payee name required by the schema."
                .into(),
            prompt: format!(
                "Raw payee: {}\nDirection: {}",
                one_line(&input.raw_payee),
                input.direction
            ),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "payee": { "type": "string", "minLength": 1 }
                },
                "required": ["payee"],
                "additionalProperties": false
            }),
        },
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
