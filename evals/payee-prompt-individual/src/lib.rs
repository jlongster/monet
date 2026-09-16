use serde::{Deserialize, Serialize};
use serde_json::json;

pub const PROMPT_VERSION: &str = "payee-prompt-individual-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub raw_payee: String,
    pub direction: String,
}

pub fn render() -> Result<String, serde_json::Error> {
    let system = "Extract the best concise payee description from a raw financial transaction payee. If it references a store or financial account, extract the store or account name. Remove transaction metadata such as dates, locations, store numbers, card digits, and reference numbers. Return only the payee name required by the schema.";
    let schema = json!({
        "type": "object",
        "properties": {
            "payee": { "type": "string", "minLength": 1 }
        },
        "required": ["payee"],
        "additionalProperties": false
    });
    Ok(format!(
        "{system}\n\nReturn only JSON matching this JSON Schema:\n{}\n",
        serde_json::to_string(&schema)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_an_invariant_payee_only_prompt() {
        let prompt = render().unwrap();
        assert!(prompt.contains("best concise payee description"));
        assert!(prompt.contains(r#""required":["payee"]"#));
        assert!(!prompt.contains("rawPayee"));
        assert!(!prompt.contains("category"));
    }
}
