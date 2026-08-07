use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    CategorizationExample, CategorizationPipeline, CategorizationQuery, CategoryAccount,
    CategoryAccountType,
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFixture {
    pub version: u32,
    pub name: String,
    pub command: String,
    pub accounts: Vec<CategoryAccount>,
    pub starting_history: Vec<CategorizationExample>,
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStep {
    pub step: usize,
    pub query: CategorizationQuery,
    pub suggested_category: Option<String>,
    pub decision: RunDecision,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDecision {
    pub action: String,
    pub input: String,
    pub account_id: Option<String>,
    pub created_account: bool,
}

pub fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/categorization/fixtures/runs")
}

pub fn import_transcript(
    transcript_path: &Path,
    name: &str,
) -> Result<(RunFixture, PathBuf), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("fixture name may contain only letters, numbers, '-' and '_'".into());
    }
    let contents = fs::read_to_string(transcript_path).map_err(|error| error.to_string())?;
    let events = contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Value>(line)
                .map_err(|error| format!("line {}: {error}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let start = events
        .iter()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("session_start"))
        .and_then(|event| event.get("data"))
        .ok_or("transcript has no session_start event")?;
    let accounts = serde_json::from_value(
        start
            .get("accounts")
            .cloned()
            .ok_or("session_start has no accounts")?,
    )
    .map_err(|error| format!("invalid starting accounts: {error}"))?;
    let starting_history = match start.get("startingHistory") {
        Some(history) => serde_json::from_value(history.clone())
            .map_err(|error| format!("invalid starting history: {error}"))?,
        None if start
            .get("seedHistory")
            .and_then(Value::as_bool)
            .unwrap_or(false) =>
        {
            let fixtures: super::CategorizationFixtures =
                serde_json::from_str(include_str!("examples.json"))
                    .map_err(|error| error.to_string())?;
            fixtures.history
        }
        None => Vec::new(),
    };
    let command = start
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| command_from_settings(start));
    let mut steps = Vec::<RunStep>::new();
    let mut positions = BTreeMap::<usize, usize>::new();
    for event in &events {
        let Some(kind) = event.get("event").and_then(Value::as_str) else {
            continue;
        };
        let Some(data) = event.get("data") else {
            continue;
        };
        if kind == "step_prompt" {
            let step = data
                .get("step")
                .and_then(Value::as_u64)
                .ok_or("step_prompt has no step")? as usize;
            let query = serde_json::from_value(
                data.get("query")
                    .cloned()
                    .ok_or("step_prompt has no query")?,
            )
            .map_err(|error| format!("step {step} has invalid query: {error}"))?;
            let suggested_category = data
                .get("suggestedAccountId")
                .or_else(|| {
                    data.get("resolution")
                        .and_then(|resolution| resolution.get("accountId"))
                })
                .and_then(Value::as_str)
                .map(str::to_owned);
            positions.insert(step, steps.len());
            steps.push(RunStep {
                step,
                query,
                suggested_category,
                decision: RunDecision {
                    action: "interrupted".into(),
                    ..RunDecision::default()
                },
            });
        } else if kind == "user_decision" {
            let step = data
                .get("step")
                .and_then(Value::as_u64)
                .ok_or("user_decision has no step")? as usize;
            let Some(position) = positions.get(&step).copied() else {
                return Err(format!("decision for unknown step {step}"));
            };
            steps[position].decision = RunDecision {
                action: data
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .into(),
                input: data
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                account_id: data
                    .get("accountId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                created_account: data
                    .get("createdAccount")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
        }
    }
    let fixture = RunFixture {
        version: 1,
        name: name.into(),
        command,
        accounts,
        starting_history,
        steps,
    };
    let directory = fixture_directory();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{name}.json"));
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&fixture).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok((fixture, path))
}

pub fn load_fixture(path: &Path) -> Result<RunFixture, String> {
    serde_json::from_str(&fs::read_to_string(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

pub fn fixture_paths(name: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let directory = fixture_directory();
    if let Some(name) = name {
        let path = directory.join(format!("{name}.json"));
        return path
            .exists()
            .then_some(vec![path])
            .ok_or_else(|| format!("fixture '{name}' does not exist"));
    }
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub fn verify_fixture(fixture: &RunFixture) -> Vec<String> {
    let mut accounts = fixture.accounts.clone();
    let mut history = fixture.starting_history.clone();
    let mut failures = Vec::new();
    for step in &fixture.steps {
        let actual = CategorizationPipeline::new(&accounts, &history).resolve(&step.query);
        let actual_suggestion = if actual.potential_transfer {
            Some("transfer".to_owned())
        } else {
            actual.account_id
        };
        if actual_suggestion != step.suggested_category {
            failures.push(format!(
                "{} step {}: expected suggestion {}, actual {} | payee={} date={} amount={} source={} ({:?})",
                fixture.name,
                step.step,
                step.suggested_category.as_deref().unwrap_or("UNCATEGORIZED"),
                actual_suggestion.as_deref().unwrap_or("UNCATEGORIZED"),
                step.query.payee,
                step.query.date,
                step.query.amount,
                step.query.source_account_id,
                step.query.source_account_type,
            ));
        }
        apply_decision(&mut accounts, &mut history, step);
    }
    failures
}

fn apply_decision(
    accounts: &mut Vec<CategoryAccount>,
    history: &mut Vec<CategorizationExample>,
    step: &RunStep,
) {
    if step.decision.action != "categorized" {
        return;
    }
    let Some(account_id) = &step.decision.account_id else {
        return;
    };
    if !accounts.iter().any(|account| account.id == *account_id) {
        accounts.push(CategoryAccount {
            id: account_id.clone(),
            name: account_id.clone(),
            account_type: if step.query.amount < 0 {
                CategoryAccountType::Expense
            } else {
                CategoryAccountType::Income
            },
        });
    }
    history.push(CategorizationExample {
        payee: step.query.payee.clone(),
        imported_category: step.query.imported_category.clone(),
        date: step.query.date.clone(),
        amount: step.query.amount,
        source_account_type: step.query.source_account_type,
        source_account_id: step.query.source_account_id.clone(),
        counterpart_account_id: account_id.clone(),
        transfer: false,
    });
}

fn command_from_settings(settings: &Value) -> String {
    let mut arguments = vec![
        "cargo run --manifest-path server/Cargo.toml --bin categorization_eval -- --interactive"
            .to_owned(),
    ];
    if let Some(payees) = settings.get("payees").and_then(Value::as_array) {
        let payees = payees
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        arguments.push(format!("--payee {}", shell_quote(&payees)));
    }
    for (field, flag) in [
        ("amountProfile", "--amount-profile"),
        ("direction", "--direction"),
        ("sourceType", "--source-type"),
        ("dateProfile", "--date-profile"),
    ] {
        if let Some(value) = settings.get(field).and_then(Value::as_str) {
            arguments.push(format!("{flag} {}", shell_quote(value)));
        }
    }
    if let Some(accounts) = settings.get("sourceAccounts").and_then(Value::as_array) {
        let accounts = accounts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        if !accounts.is_empty() {
            arguments.push(format!("--source-account {}", shell_quote(&accounts)));
        }
    }
    if let Some(amount) = settings.get("fixedAmount").and_then(Value::as_i64) {
        arguments.push(format!("--amount {}", format_cents(amount)));
    }
    if let Some(date) = settings.get("fixedDate").and_then(Value::as_str) {
        arguments.push(format!("--date {}", shell_quote(date)));
    }
    for (field, flag) in [("steps", "--steps"), ("seed", "--seed")] {
        if let Some(value) = settings.get(field).and_then(Value::as_u64) {
            arguments.push(format!("{flag} {value}"));
        }
    }
    if settings
        .get("seedHistory")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        arguments.push("--seed-history".into());
    }
    arguments.join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let absolute = i128::from(cents).abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}
