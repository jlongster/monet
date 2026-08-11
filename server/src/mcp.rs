use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    model::{AccountCursor, AccountType},
    state::MonetState,
};

const PROTOCOL_VERSION: &str = "2025-03-26";

pub(crate) async fn handle(
    State(state): State<MonetState>,
    Json(payload): Json<Value>,
) -> Response {
    if let Some(requests) = payload.as_array() {
        let responses = requests
            .iter()
            .filter_map(|request| dispatch(&state, request))
            .collect::<Vec<_>>();
        if responses.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        return Json(Value::Array(responses)).into_response();
    }
    match dispatch(&state, &payload) {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn dispatch(state: &MonetState, request: &Value) -> Option<Value> {
    let object = match request.as_object() {
        Some(object) if object.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => object,
        _ => return Some(error(Value::Null, -32600, "Invalid Request")),
    };
    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str);
    if method.is_some_and(|method| method.starts_with("notifications/")) {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        Some("initialize") => Ok(initialize(&params)),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({ "tools": tools() })),
        Some("tools/call") => call_tool(state, &params),
        Some(_) => return Some(error(id, -32601, "Method not found")),
        None => return Some(error(id, -32600, "Invalid Request")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => error(id, -32602, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    let protocol_version = if matches!(requested, "2025-03-26" | "2024-11-05") {
        requested
    } else {
        PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "monet",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn tools() -> Vec<Value> {
    let annotations = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    });
    vec![
        json!({
            "name": "monet_list_accounts",
            "description": "List Monet accounts with balances and hierarchy metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "types": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["asset", "liability", "income", "expense", "equity"]
                        }
                    }
                },
                "additionalProperties": false
            },
            "annotations": annotations
        }),
        json!({
            "name": "monet_list_transactions",
            "description": "List projected transaction postings, optionally scoped to an account, account types, or search text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "accountId": { "type": "string" },
                    "accountTypes": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["asset", "liability", "income", "expense", "equity"]
                        }
                    },
                    "search": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                    "cursor": {
                        "type": "object",
                        "properties": {
                            "date": { "type": "string" },
                            "sourceId": { "type": "string" },
                            "postingId": { "type": "string" }
                        },
                        "required": ["date", "sourceId", "postingId"],
                        "additionalProperties": false
                    }
                },
                "additionalProperties": false
            },
            "annotations": annotations
        }),
        json!({
            "name": "monet_get_transaction",
            "description": "Get one transaction source, projected postings, changes, and activity logs by source ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sourceId": { "type": "string" }
                },
                "required": ["sourceId"],
                "additionalProperties": false
            },
            "annotations": annotations
        }),
    ]
}

fn call_tool(state: &MonetState, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call requires a tool name")?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "monet_list_accounts" => list_accounts(state, arguments),
        "monet_list_transactions" => list_transactions(state, arguments),
        "monet_get_transaction" => get_transaction(state, arguments),
        _ => return Ok(tool_error(&format!("Unknown tool: {name}"))),
    };
    Ok(match result {
        Ok((message, structured)) => tool_result(&message, structured),
        Err(error) => tool_error(&error),
    })
}

#[derive(Deserialize)]
struct ListAccountsInput {
    #[serde(default)]
    types: Vec<AccountType>,
}

fn list_accounts(state: &MonetState, arguments: Value) -> Result<(String, Value), String> {
    let input: ListAccountsInput = serde_json::from_value(arguments).map_err(|e| e.to_string())?;
    let types = input.types;
    let database = state
        .database
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let accounts = database
        .list_accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|account| types.is_empty() || types.contains(&account.account_type))
        .collect::<Vec<_>>();
    Ok((
        format!("Found {} accounts.", accounts.len()),
        json!({ "accounts": accounts }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListTransactionsInput {
    account_id: Option<String>,
    #[serde(default)]
    account_types: Vec<AccountType>,
    search: Option<String>,
    limit: Option<usize>,
    cursor: Option<McpCursor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpCursor {
    date: String,
    source_id: String,
    posting_id: String,
}

fn list_transactions(state: &MonetState, arguments: Value) -> Result<(String, Value), String> {
    let input: ListTransactionsInput =
        serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    let limit = input.limit.unwrap_or(50).clamp(1, 200);
    let cursor = input.cursor.map(|cursor| AccountCursor {
        date: cursor.date,
        source_id: cursor.source_id,
        posting_id: cursor.posting_id,
    });
    let database = state
        .database
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let page = match input.account_id {
        Some(account_id) => {
            database.list_account(&account_id, limit, cursor.as_ref(), input.search.as_deref())
        }
        None => database.list_all_transactions(
            limit,
            cursor.as_ref(),
            (!input.account_types.is_empty()).then_some(input.account_types.as_slice()),
            input.search.as_deref(),
        ),
    }
    .map_err(|error| error.to_string())?;
    let next = page.next.as_ref().map(|cursor| {
        json!({
            "date": cursor.date,
            "sourceId": cursor.source_id,
            "postingId": cursor.posting_id,
        })
    });
    Ok((
        format!("Found {} transaction postings.", page.transactions.len()),
        json!({ "transactions": page.transactions, "next": next }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetTransactionInput {
    source_id: String,
}

fn get_transaction(state: &MonetState, arguments: Value) -> Result<(String, Value), String> {
    let input: GetTransactionInput =
        serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    let database = state
        .database
        .lock()
        .map_err(|_| "database lock poisoned")?;
    let transaction = database
        .get_source(&input.source_id)
        .map_err(|error| error.to_string())?;
    Ok((
        format!("Loaded transaction {}.", input.source_id),
        json!({ "transaction": transaction }),
    ))
}

fn tool_result(message: &str, structured: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ConfigFile, db::Database};

    fn state() -> MonetState {
        let path = std::env::temp_dir().join(format!("monet-mcp-{}.json", uuid::Uuid::now_v7()));
        MonetState::new(
            Database::open(":memory:").unwrap(),
            ConfigFile::at(path).unwrap(),
        )
    }

    #[test]
    fn initializes_and_lists_read_only_tools() {
        let state = state();
        let initialized = dispatch(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-03-26" }
            }),
        )
        .unwrap();
        assert_eq!(initialized["result"]["protocolVersion"], "2025-03-26");

        let listed = dispatch(
            &state,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert!(
            tools
                .iter()
                .all(|tool| tool["annotations"]["readOnlyHint"] == true)
        );
    }

    #[test]
    fn calls_list_accounts_with_structured_content() {
        let state = state();
        let response = dispatch(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": "accounts",
                "method": "tools/call",
                "params": { "name": "monet_list_accounts", "arguments": {} }
            }),
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(
            response["result"]["structuredContent"]["accounts"],
            json!([])
        );
    }

    #[test]
    fn initialized_notification_has_no_response() {
        assert!(
            dispatch(
                &state(),
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .is_none()
        );
    }
}
