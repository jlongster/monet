use std::{
    collections::{HashMap, HashSet},
    env,
};

use serde::Deserialize;
use serde_json::json;

use crate::{
    app_log,
    db::{Database, DatabaseError},
    error::MonetError,
    model::{
        AccountInput, AccountRecord, AccountType, Change, ImportSummary, ImportTransactionsInput,
        ImportedTransaction, InferenceGenerateInput, ModelError, PayeeFormat,
    },
    plugin::PluginError,
    state::MonetState,
};
use monet_server::{
    categorization::{
        CategorizationMethod, CategorizationPipeline, CategoryAccount, CategoryAccountType,
    },
    payee_resolution::{PayeeResolver, clean_payee_name, normalize_payee},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitialTransferReview {
    not_transfers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingPayeeResolution {
    source_ids: Vec<String>,
    imported_payee: String,
    potential_payee: Option<String>,
    amount_direction: String,
    local_reason: String,
}

struct PayeePromptLine {
    id: String,
    raw_payee: String,
    potential_payee: Option<String>,
    direction: String,
}

#[derive(Debug, PartialEq, Eq)]
struct PayeeCategory {
    alias: String,
    account_id: String,
    label: String,
    account_type: CategoryAccountType,
}

#[derive(Debug, PartialEq, Eq)]
struct InferredPayee {
    source_id: String,
    imported_payee: String,
    payee: String,
    local_reason: String,
    resolution_source: String,
    category_account_id: Option<String>,
    potential_transfer: bool,
}

struct ImportedAccountSnapshot {
    account: AccountRecord,
    pending_ids: Vec<String>,
}

struct QueuedImportedTransaction {
    account_index: usize,
    transaction: ImportedTransaction,
}

const OPENING_BALANCES_ACCOUNT_ID: &str = "b699843f-11bd-54e8-888b-452cb32be97b";
const PAYEE_INFERENCE_BATCH_SIZE: usize = 100;
const DEFAULT_TRANSACTION_BATCH_SIZE: usize = 200;
fn require_import_config(
    state: &MonetState,
) -> Result<crate::model::ImportPluginConfig, MonetError> {
    state
        .config
        .load()?
        .import
        .ok_or_else(|| PluginError::NotConfigured.into())
}

pub(crate) async fn run_import(
    state: &MonetState,
    end_date: Option<String>,
) -> Result<ImportSummary, MonetError> {
    if let Ok(mut log) = state.import_log.lock() {
        log.running = true;
        log.lines.clear();
        log.lines.push("Starting transaction sync".into());
    }
    app_log::write("sync", "Starting transaction sync");
    let result = run_import_inner(state, end_date).await;
    let completion = match &result {
        Ok(summary) => format!(
            "Sync complete: {} accounts, {} transactions, {} payees resolved, {} categorized, {} pending removed",
            summary.accounts,
            summary.transactions,
            summary.payees_resolved,
            summary.categorized,
            summary.deleted_pending
        ),
        Err(error) => format!("Sync failed: {error:?}"),
    };
    app_log::write("sync", &completion);
    if let Ok(mut log) = state.import_log.lock() {
        log.running = false;
        log.lines.push(completion);
    }
    result
}

fn log_import(state: &MonetState, message: impl Into<String>) {
    let message = message.into();
    app_log::write("sync", &message);
    if let Ok(mut log) = state.import_log.lock() {
        log.lines.push(message);
    }
}

async fn run_import_inner(
    state: &MonetState,
    end_date: Option<String>,
) -> Result<ImportSummary, MonetError> {
    log_import(state, "Loading transaction import plugin configuration");
    let config = require_import_config(&state)?;
    log_import(state, "Requesting account snapshot from import plugin");
    let (imported_accounts, payee_format) = state.importer.accounts(config.clone()).await?;
    log_import(
        state,
        format!(
            "Received {} imported accounts",
            imported_accounts.accounts.len()
        ),
    );
    let accounts = {
        let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        database.sync_accounts(&imported_accounts.accounts)?
    };
    let resolve_payees = payee_format == PayeeFormat::Raw;
    if !resolve_payees {
        log_import(
            state,
            "Importer provides clean payees; skipping payee resolution",
        );
    }
    let new_account_count = accounts.iter().filter(|(_, initial)| *initial).count();
    log_import(
        state,
        format!(
            "Registered {} accounts ({} new)",
            accounts.len(),
            new_account_count
        ),
    );

    let mut summary = ImportSummary {
        accounts: accounts.len(),
        transactions: 0,
        deleted_pending: 0,
        categorized: 0,
        payees_resolved: 0,
    };
    let transaction_batch_size = configured_transaction_batch_size();
    log_import(
        state,
        format!("Processing transactions in global batches of {transaction_batch_size}"),
    );
    let mut processed_source_ids = Vec::new();
    let mut new_source_ids = Vec::new();
    let mut llm_category_suggestions = HashMap::new();
    let mut llm_potential_transfers = HashSet::new();
    let categories = {
        let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        payee_categories(&database.list_accounts()?)
    };
    let category_accounts = categories
        .iter()
        .map(|category| CategoryAccount {
            id: category.account_id.clone(),
            name: category.label.clone(),
            account_type: category.account_type,
        })
        .collect::<Vec<_>>();
    let mut snapshots = Vec::with_capacity(accounts.len());
    let mut queued_transactions = Vec::new();
    for (account, initial) in accounts {
        let account_id = account
            .external_id
            .as_deref()
            .ok_or_else(|| DatabaseError::AccountNotFound(account.id.clone()))?;
        let start_date = if let Some(end_date) = end_date.as_deref() {
            let year = end_date
                .get(..4)
                .filter(|year| year.chars().all(|character| character.is_ascii_digit()))
                .ok_or_else(|| DatabaseError::from(ModelError::InvalidSource))?;
            Some(format!("{year}-01-01"))
        } else if initial {
            None
        } else {
            Some({
                let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
                database.account_sync_start_date(&account.id)?
            })
        };
        log_import(
            state,
            if initial {
                format!("{}: requesting initial transaction snapshot", account.name)
            } else {
                format!(
                    "{}: requesting transactions from {}",
                    account.name,
                    start_date.as_deref().unwrap_or("derived date")
                )
            },
        );
        let mut imported = state
            .importer
            .transactions(
                config.clone(),
                ImportTransactionsInput {
                    account_id: account_id.to_owned(),
                    initial,
                    start_date,
                    end_date: end_date.clone(),
                },
            )
            .await?;
        let returned = imported.transactions.len();
        Database::validate_transaction_snapshot(&imported)?;
        let superseded_pending = remove_superseded_pending(&mut imported.transactions);
        if superseded_pending > 0 {
            log_import(
                state,
                format!(
                    "{}: omitted {superseded_pending} pending transactions replaced by posted transactions",
                    account.name
                ),
            );
        }
        let account_index = snapshots.len();
        queued_transactions.extend(imported.transactions.into_iter().map(|transaction| {
            QueuedImportedTransaction {
                account_index,
                transaction,
            }
        }));
        snapshots.push(ImportedAccountSnapshot {
            account,
            pending_ids: imported.pending_ids,
        });
        log_import(
            state,
            format!(
                "{}: received {returned} transactions",
                snapshots[account_index].account.name
            ),
        );
    }
    sort_queued_transactions(&mut queued_transactions, &snapshots);

    let batch_count = queued_transactions.len().div_ceil(transaction_batch_size);
    for (batch_index, batch) in queued_transactions
        .chunks(transaction_batch_size)
        .enumerate()
    {
        let resolver = if resolve_payees {
            let examples = {
                let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
                database.payee_examples()?
            };
            log_import(
                state,
                format!(
                    "Batch {}/{}: {} transactions, {} payee examples",
                    batch_index + 1,
                    batch_count,
                    batch.len(),
                    examples.len()
                ),
            );
            Some(PayeeResolver::new(&examples))
        } else {
            log_import(
                state,
                format!(
                    "Batch {}/{}: {} transactions",
                    batch_index + 1,
                    batch_count,
                    batch.len()
                ),
            );
            None
        };
        let items = batch
            .iter()
            .map(|item| (&snapshots[item.account_index].account, &item.transaction))
            .collect::<Vec<_>>();
        let result = {
            let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            database.sync_transaction_batch(&items, resolver.as_ref())?
        };
        summary.transactions += result.transactions;
        summary.payees_resolved += result.resolved_payees.len();
        for resolved in result.resolved_payees {
            log_import(
                state,
                format!(
                    "Payee local: {} -> {}",
                    prompt_line(&resolved.imported_payee),
                    prompt_line(&resolved.resolved_payee)
                ),
            );
        }
        processed_source_ids.extend(result.processed_source_ids);
        new_source_ids.extend(result.new_source_ids);
        let mut pending_positions = HashMap::<String, usize>::new();
        let mut unresolved_payees = Vec::<PendingPayeeResolution>::new();
        for unresolved in result.unresolved_payees {
            let key = format!(
                "{}\0{}",
                normalize_payee(&unresolved.imported_payee),
                unresolved.amount_direction
            );
            if let Some(index) = pending_positions.get(&key).copied() {
                unresolved_payees[index]
                    .source_ids
                    .push(unresolved.source_id);
            } else {
                pending_positions.insert(key, unresolved_payees.len());
                unresolved_payees.push(PendingPayeeResolution {
                    source_ids: vec![unresolved.source_id],
                    imported_payee: unresolved.imported_payee,
                    potential_payee: unresolved.potential_payee,
                    amount_direction: unresolved.amount_direction,
                    local_reason: unresolved.local_reason,
                });
            }
        }
        let (inferred_payees, suggested_categories, potential_transfers) =
            resolve_imported_payees(state, &unresolved_payees, &categories).await?;
        summary.payees_resolved += inferred_payees;
        llm_category_suggestions.extend(suggested_categories);
        llm_potential_transfers.extend(potential_transfers);
    }

    summary.deleted_pending = {
        let cleanup = snapshots
            .iter()
            .map(|snapshot| (&snapshot.account, snapshot.pending_ids.as_slice()))
            .collect::<Vec<_>>();
        let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        database.cleanup_pending_transactions(&cleanup)?
    };
    log_import(
        state,
        format!(
            "Import complete; {} new sources need counterparts",
            new_source_ids.len()
        ),
    );
    summary.categorized += process_structural_categorization(state, &processed_source_ids).await?;
    let history = {
        let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        database.categorization_examples()?
    };
    log_import(
        state,
        format!(
            "Categorizing {} transactions from one snapshot with {} examples",
            processed_source_ids.len(),
            history.len()
        ),
    );
    let categorizer = CategorizationPipeline::new(&category_accounts, &history);
    let new_source_ids = new_source_ids.iter().cloned().collect::<HashSet<_>>();
    summary.categorized += categorize_sources(
        state,
        &categorizer,
        &processed_source_ids,
        &llm_category_suggestions,
        &llm_potential_transfers,
        &new_source_ids,
    )?;
    Ok(summary)
}

fn sort_queued_transactions(
    transactions: &mut [QueuedImportedTransaction],
    accounts: &[ImportedAccountSnapshot],
) {
    transactions.sort_by(|left, right| {
        left.transaction
            .date
            .cmp(&right.transaction.date)
            .then_with(|| {
                accounts[left.account_index]
                    .account
                    .id
                    .cmp(&accounts[right.account_index].account.id)
            })
            .then_with(|| left.transaction.id.cmp(&right.transaction.id))
    });
}

fn configured_transaction_batch_size() -> usize {
    env::var("MONET_TRANSACTION_BATCH_SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TRANSACTION_BATCH_SIZE)
}

fn remove_superseded_pending(transactions: &mut Vec<ImportedTransaction>) -> usize {
    let replaced = transactions
        .iter()
        .filter_map(|transaction| transaction.replaces_pending_id.clone())
        .collect::<HashSet<_>>();
    let before = transactions.len();
    transactions
        .retain(|transaction| !transaction.pending || !replaced.contains(transaction.id.as_str()));
    before - transactions.len()
}

async fn resolve_imported_payees(
    state: &MonetState,
    unresolved: &[PendingPayeeResolution],
    categories: &[PayeeCategory],
) -> Result<(usize, HashMap<String, String>, HashSet<String>), MonetError> {
    if unresolved.is_empty() {
        return Ok((0, HashMap::new(), HashSet::new()));
    }
    let unresolved_sources = unresolved
        .iter()
        .map(|pending| pending.source_ids.len())
        .sum::<usize>();
    let inference = match state.config.load()?.inference {
        Some(config) => config,
        None => {
            log_import(
                state,
                format!(
                    "Inference is not configured; applying {unresolved_sources} fallback payees"
                ),
            );
            let fallback = fallback_payees(unresolved);
            let resolved = persist_inferred_payees(state, fallback)?;
            return Ok(resolved);
        }
    };
    let mut resolved = 0;
    let mut suggested_categories = HashMap::new();
    let mut potential_transfers = HashSet::new();
    for batch in unresolved.chunks(PAYEE_INFERENCE_BATCH_SIZE) {
        log_import(
            state,
            format!("Sending {} unresolved payees to inference", batch.len()),
        );
        for category in categories {
            log_import(
                state,
                format!("Payee LLM category: {}: {}", category.alias, category.label),
            );
        }
        for line in payee_inference_lines(batch) {
            log_import(state, format!("Payee LLM input: {line}"));
        }
        let input = payee_inference_input(batch, categories);
        let response = match state.inference.generate(inference.clone(), input).await {
            Ok(response) => response,
            Err(error) => {
                log_import(state, format!("Payee inference failed: {error}"));
                let fallback = fallback_payees(batch);
                let persisted = persist_inferred_payees(state, fallback)?;
                resolved += persisted.0;
                suggested_categories.extend(persisted.1);
                potential_transfers.extend(persisted.2);
                continue;
            }
        };
        if let Some(object) = response
            .structured
            .as_ref()
            .and_then(serde_json::Value::as_object)
        {
            for index in 0..batch.len() {
                let id = format!("p{}", index + 1);
                let result = format_inferred_payee(object.get(&id), categories);
                log_import(state, format!("Payee LLM result: {id}: {result}"));
            }
        } else {
            log_import(state, "Payee LLM result: <missing structured output>");
        }
        let Some(inferred) = response
            .structured
            .as_ref()
            .and_then(|value| parse_inferred_payees(value, batch, categories))
        else {
            log_import(
                state,
                "Payee inference returned incomplete or invalid identifiers",
            );
            let fallback = fallback_payees(batch);
            let persisted = persist_inferred_payees(state, fallback)?;
            resolved += persisted.0;
            suggested_categories.extend(persisted.1);
            potential_transfers.extend(persisted.2);
            continue;
        };
        let persisted = persist_inferred_payees(state, inferred)?;
        resolved += persisted.0;
        suggested_categories.extend(persisted.1);
        potential_transfers.extend(persisted.2);
    }
    log_import(
        state,
        format!("Inference resolved {resolved} of {unresolved_sources} unmatched payees"),
    );
    Ok((resolved, suggested_categories, potential_transfers))
}

fn fallback_payees(batch: &[PendingPayeeResolution]) -> Vec<InferredPayee> {
    batch
        .iter()
        .flat_map(|pending| {
            let payee = clean_payee_name(&pending.imported_payee);
            pending
                .source_ids
                .iter()
                .map(move |source_id| InferredPayee {
                    source_id: source_id.clone(),
                    imported_payee: pending.imported_payee.clone(),
                    payee: payee.clone(),
                    local_reason: pending.local_reason.clone(),
                    resolution_source: "fallback".into(),
                    category_account_id: None,
                    potential_transfer: false,
                })
        })
        .collect()
}

fn persist_inferred_payees(
    state: &MonetState,
    inferred: Vec<InferredPayee>,
) -> Result<(usize, HashMap<String, String>, HashSet<String>), MonetError> {
    for payee in &inferred {
        log_import(state, format!("Payee fallback/result: {}", payee.payee));
    }
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    let payee_resolutions = inferred
        .iter()
        .map(|inferred| (inferred.source_id.clone(), inferred.payee.clone()))
        .collect::<Vec<_>>();
    let resolved = database.append_payee_resolutions(&payee_resolutions)?;
    let payee_logs = inferred
        .iter()
        .map(|inferred| {
            (
                inferred.source_id.clone(),
                format!(
                    "Payee: {} -> {} ({}; local: {})",
                    prompt_line(&inferred.imported_payee),
                    inferred.payee,
                    inferred.resolution_source,
                    inferred.local_reason
                ),
            )
        })
        .collect::<Vec<_>>();
    let logs = payee_logs;
    database.append_transaction_logs(&logs)?;
    let mut suggested_categories = HashMap::new();
    let mut potential_transfers = HashSet::new();
    for inferred in inferred {
        if let Some(account_id) = inferred.category_account_id {
            suggested_categories.insert(inferred.source_id.clone(), account_id);
        }
        if inferred.potential_transfer {
            potential_transfers.insert(inferred.source_id);
        }
    }
    Ok((resolved, suggested_categories, potential_transfers))
}

fn payee_inference_input(
    batch: &[PendingPayeeResolution],
    categories: &[PayeeCategory],
) -> InferenceGenerateInput {
    let payees = batch
        .iter()
        .enumerate()
        .map(|(index, pending)| PayeePromptLine {
            id: format!("p{}", index + 1),
            raw_payee: pending.imported_payee.clone(),
            potential_payee: pending.potential_payee.clone(),
            direction: pending.amount_direction.clone(),
        })
        .collect::<Vec<_>>();
    let rendered = monet_server::payee_prompt::render(monet_server::payee_prompt::RenderInvocation {
        input: monet_server::payee_prompt::EvalInput {
            accounts: categories
                .iter()
                .map(|category| monet_server::payee_prompt::Account {
                    id: category.account_id.clone(),
                    parent_id: None,
                    name: category
                        .label
                        .split_once(": ")
                        .map_or(category.label.clone(), |(_, name)| name.into()),
                    account_type: match category.account_type {
                        CategoryAccountType::Income => "income",
                        CategoryAccountType::Expense => "expense",
                    }
                    .into(),
                })
                .collect(),
            payees: payees
                .into_iter()
                .map(|payee| monet_server::payee_prompt::Payee {
                    id: payee.id,
                    raw_payee: payee.raw_payee,
                    direction: payee.direction,
                    potential_payee: payee.potential_payee,
                })
                .collect(),
        },
        parameters: monet_server::payee_prompt::Parameters {
            include_potentials: true,
        },
    });
    InferenceGenerateInput {
        model: None,
        system: Some(rendered.request.system),
        prompt: rendered.request.prompt,
        output_schema: Some(rendered.request.output_schema),
        timeout_ms: None,
    }
}

fn payee_prompt_lines(payees: &[PayeePromptLine]) -> Vec<String> {
    payees
        .iter()
        .map(|payee| {
            let details = payee
                .potential_payee
                .as_deref()
                .map(|potential| {
                    format!(
                        "potential: {}; direction: {}",
                        prompt_line(potential),
                        payee.direction
                    )
                })
                .unwrap_or_else(|| format!("direction: {}", payee.direction));
            format!(
                "{}: {} ({details})",
                payee.id,
                prompt_line(&payee.raw_payee)
            )
        })
        .collect()
}

fn payee_inference_lines(batch: &[PendingPayeeResolution]) -> Vec<String> {
    let payees = batch
        .iter()
        .enumerate()
        .map(|(index, pending)| PayeePromptLine {
            id: format!("p{}", index + 1),
            raw_payee: pending.imported_payee.clone(),
            potential_payee: pending.potential_payee.clone(),
            direction: pending.amount_direction.clone(),
        })
        .collect::<Vec<_>>();
    payee_prompt_lines(&payees)
}

fn parse_inferred_payees(
    structured: &serde_json::Value,
    batch: &[PendingPayeeResolution],
    categories: &[PayeeCategory],
) -> Option<Vec<InferredPayee>> {
    let object = structured.as_object()?;
    if object.len() != batch.len() {
        return None;
    }
    let category_ids = categories
        .iter()
        .map(|category| (category.alias.as_str(), category.account_id.as_str()))
        .collect::<HashMap<_, _>>();
    batch
        .iter()
        .enumerate()
        .map(|(index, pending)| {
            let id = format!("p{}", index + 1);
            let value = object.get(&id)?;
            let result = value.as_object()?;
            let payee = result.get("payee")?.as_str()?.trim();
            let (category_account_id, potential_transfer) = match result.get("category")? {
                serde_json::Value::Null => (None, false),
                serde_json::Value::String(alias) if alias == "transfer" => (None, true),
                serde_json::Value::String(alias) => {
                    (Some((*category_ids.get(alias.as_str())?).to_owned()), false)
                }
                _ => return None,
            };
            let payee = clean_payee_name(payee);
            (!payee.is_empty()).then(|| {
                pending
                    .source_ids
                    .iter()
                    .map(|source_id| InferredPayee {
                        source_id: source_id.clone(),
                        imported_payee: pending.imported_payee.clone(),
                        payee: payee.clone(),
                        local_reason: pending.local_reason.clone(),
                        resolution_source: "LLM".into(),
                        category_account_id: category_account_id.clone(),
                        potential_transfer,
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect::<Option<Vec<_>>>()
        .map(|groups| groups.into_iter().flatten().collect())
}

fn format_inferred_payee(
    value: Option<&serde_json::Value>,
    categories: &[PayeeCategory],
) -> String {
    let Some(value) = value else {
        return "<missing or invalid>".into();
    };
    let Some(result) = value.as_object() else {
        return "<missing or invalid>".into();
    };
    let Some(payee) = result.get("payee").and_then(serde_json::Value::as_str) else {
        return "<missing or invalid>".into();
    };
    let category = result
        .get("category")
        .and_then(serde_json::Value::as_str)
        .and_then(|alias| categories.iter().find(|category| category.alias == alias));
    if result.get("category").and_then(serde_json::Value::as_str) == Some("transfer") {
        return format!("{} (category: transfer)", clean_payee_name(payee));
    }
    match category {
        Some(category) => format!("{} (category: {})", clean_payee_name(payee), category.label),
        None => clean_payee_name(payee),
    }
}

fn payee_categories(accounts: &[AccountRecord]) -> Vec<PayeeCategory> {
    let by_id = accounts
        .iter()
        .map(|account| (account.id.as_str(), account))
        .collect::<HashMap<_, _>>();
    accounts
        .iter()
        .filter(|account| {
            matches!(
                account.account_type,
                AccountType::Expense | AccountType::Income
            ) && !(account.parent_id.is_none() && account.has_children)
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
            PayeeCategory {
                alias: format!("c{}", index + 1),
                account_id: account.id.clone(),
                label: format!("{}: {}", account.account_type.as_str(), names.join(" > ")),
                account_type: if account.account_type == AccountType::Income {
                    CategoryAccountType::Income
                } else {
                    CategoryAccountType::Expense
                },
            }
        })
        .collect()
}

fn categorize_sources(
    state: &MonetState,
    pipeline: &CategorizationPipeline,
    source_ids: &[String],
    suggested_categories: &HashMap<String, String>,
    potential_transfers: &HashSet<String>,
    new_source_ids: &HashSet<String>,
) -> Result<usize, MonetError> {
    let queries = {
        let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        source_ids
            .iter()
            .filter(|source_id| !potential_transfers.contains(source_id.as_str()))
            .map(|source_id| {
                database
                    .categorization_query(source_id)
                    .map(|query| (source_id.clone(), query))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut transfer_notes = potential_transfers
        .intersection(new_source_ids)
        .map(|source_id| (source_id.clone(), "LLM suggested transfer".into()))
        .collect::<HashMap<String, String>>();
    let mut decisions = Vec::new();
    let mut category_logs = Vec::new();
    for (source_id, query) in queries {
        let Some(query) = query else {
            continue;
        };
        let resolution = pipeline.resolve_with_suggestion(
            &query,
            suggested_categories.get(&source_id).map(String::as_str),
        );
        if resolution.potential_transfer {
            if new_source_ids.contains(&source_id) {
                transfer_notes.insert(source_id.clone(), resolution.reason);
            }
            continue;
        }
        if let Some(account_id) = resolution.account_id {
            decisions.push((
                source_id,
                query.payee,
                account_id,
                resolution.account_name.unwrap_or_default(),
                resolution.method,
                resolution.reason,
            ));
        } else if new_source_ids.contains(&source_id) {
            let considered = resolution.candidates.first().map(|candidate| {
                format!(
                    "; considered {} at {:.0}%",
                    candidate.account_name,
                    candidate.agreement * 100.0
                )
            });
            category_logs.push((
                source_id,
                format!(
                    "Category: deferred{} ({})",
                    considered.unwrap_or_default(),
                    resolution.reason
                ),
            ));
        }
    }
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    for (source_id, reason) in transfer_notes {
        if mark_potential_transfer(
            &mut database,
            &source_id,
            &format!("Category: Potential transfer ({reason})"),
        )? {
            log_import(
                state,
                format!("Category deferred: {source_id} marked Potential transfer"),
            );
        }
    }
    let mut categorized = 0;
    for (source_id, payee, account_id, account_name, method, reason) in decisions {
        if assign_counterpart(&mut database, &source_id, &account_id)? {
            categorized += 1;
            if new_source_ids.contains(&source_id) {
                category_logs.push((
                    source_id.clone(),
                    format!("Category: {account_name} ({reason})"),
                ));
            }
            log_import(
                state,
                format!(
                    "Category {}: {} -> {}",
                    match method {
                        CategorizationMethod::ExactHistory => "exact history",
                        CategorizationMethod::LlmFallback => "LLM suggestion",
                    },
                    prompt_line(&payee),
                    account_name
                ),
            );
        }
    }
    database.append_transaction_logs(&category_logs)?;
    Ok(categorized)
}

fn mark_potential_transfer(
    database: &mut Database,
    source_id: &str,
    log_message: &str,
) -> Result<bool, DatabaseError> {
    let bundle = database.get_source(source_id)?;
    if bundle.projection.postings.len() != 1 || bundle.projection.unallocated_amount == 0 {
        return Ok(false);
    }
    database.append_changes(
        source_id,
        vec![
            Change::Set {
                target_id: source_id.into(),
                field: "notes".into(),
                value: json!("Potential transfer"),
            },
            Change::Log {
                message: log_message.into(),
            },
        ],
    )?;
    Ok(true)
}

fn clear_potential_transfer_note(
    database: &mut Database,
    source_id: &str,
) -> Result<bool, DatabaseError> {
    let bundle = database.get_source(source_id)?;
    if bundle
        .projection
        .postings
        .first()
        .and_then(|posting| posting.posting.notes.as_deref())
        != Some("Potential transfer")
    {
        return Ok(false);
    }
    database.append_changes(
        source_id,
        vec![Change::Set {
            target_id: source_id.into(),
            field: "notes".into(),
            value: serde_json::Value::Null,
        }],
    )?;
    Ok(true)
}

fn prompt_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn process_structural_categorization(
    state: &MonetState,
    processed_source_ids: &[String],
) -> Result<usize, MonetError> {
    let mut handled = 0;
    if !processed_source_ids.is_empty() {
        let opening = {
            let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            processed_source_ids
                .iter()
                .map(|source_id| database.get_source(source_id))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|bundle| bundle.source.transaction.opening_balance)
                .collect::<Vec<_>>()
        };
        if !opening.is_empty() {
            log_import(
                state,
                format!("Assigning {} opening balances to equity", opening.len()),
            );
            let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            if !database
                .list_accounts()?
                .iter()
                .any(|account| account.id == OPENING_BALANCES_ACCOUNT_ID)
            {
                database.upsert_account(
                    OPENING_BALANCES_ACCOUNT_ID,
                    AccountInput {
                        parent_id: None,
                        name: "Opening Balances".into(),
                        account_type: AccountType::Equity,
                    },
                )?;
            }
            for bundle in opening {
                if assign_counterpart(
                    &mut database,
                    &bundle.source.id,
                    OPENING_BALANCES_ACCOUNT_ID,
                )? {
                    handled += 1;
                    database.append_transaction_logs(&[(
                        bundle.source.id.clone(),
                        "Category: Opening Balances (opening balance)".into(),
                    )])?;
                }
            }
        }
    }

    if processed_source_ids.is_empty() {
        return Ok(handled);
    }
    let inference = match state.config.load()?.inference {
        Some(config) => config,
        None => {
            log_import(
                state,
                "Inference is not configured; skipping transfer review",
            );
            return Ok(handled);
        }
    };
    let (accounts, pairs) = {
        let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        let accounts = database
            .list_accounts()?
            .into_iter()
            .filter(|account| {
                matches!(
                    account.account_type,
                    AccountType::Asset | AccountType::Liability
                )
            })
            .collect::<Vec<_>>();
        (
            accounts,
            database.transfer_pairs_for_sources(processed_source_ids)?,
        )
    };
    if pairs.is_empty() {
        log_import(state, "Transfer review found no candidate pairs");
        return Ok(handled);
    }

    let account_id_to_alias = accounts
        .iter()
        .enumerate()
        .map(|(index, account)| (account.id.clone(), format!("a{index}")))
        .collect::<HashMap<_, _>>();
    let mut accounts_csv = vec![csv_row(&["account", "type", "parent", "name"])];
    for account in &accounts {
        accounts_csv.push(csv_row(&[
            &account_id_to_alias[&account.id],
            account.account_type.as_str(),
            account
                .parent_id
                .as_ref()
                .and_then(|id| account_id_to_alias.get(id))
                .map(String::as_str)
                .unwrap_or(""),
            &account.name,
        ]));
    }
    let mut candidates_csv = vec![csv_row(&[
        "candidate",
        "left_account",
        "left_date",
        "left_amount",
        "left_payee",
        "right_account",
        "right_date",
        "right_amount",
        "right_payee",
    ])];
    for (index, (left, right)) in pairs.iter().enumerate() {
        candidates_csv.push(csv_row(&[
            &format!("p{index}"),
            left.source
                .transaction
                .account_id
                .as_ref()
                .and_then(|id| account_id_to_alias.get(id))
                .map(String::as_str)
                .unwrap_or(""),
            &left.source.transaction.date,
            &decimal_amount(left.source.transaction.amount),
            left.source.transaction.payee.as_deref().unwrap_or(""),
            right
                .source
                .transaction
                .account_id
                .as_ref()
                .and_then(|id| account_id_to_alias.get(id))
                .map(String::as_str)
                .unwrap_or(""),
            &right.source.transaction.date,
            &decimal_amount(right.source.transaction.amount),
            right.source.transaction.payee.as_deref().unwrap_or(""),
        ]));
    }
    log_import(
        state,
        format!(
            "Transfer review: {} asset/liability accounts, {} candidate pairs",
            accounts.len(),
            pairs.len()
        ),
    );
    let response = state
        .inference
        .generate(
            inference,
            InferenceGenerateInput {
                model: None,
                system: Some(
                    "Review every candidate pair in the CSV. All pairs are assumed to be transfers unless you explicitly reject them. Return only candidate aliases that should NOT be reconciled as transfers. Reject coincidental opposite amounts, refunds, reversals, fees, or unrelated transactions."
                        .into(),
                ),
                prompt: format!(
                    "ASSET AND LIABILITY ACCOUNTS CSV\n{}\n\nPOSSIBLE TRANSFERS CSV\n{}",
                    accounts_csv.join("\n"),
                    candidates_csv.join("\n")
                ),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "notTransfers": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["notTransfers"],
                    "additionalProperties": false
                })),
                timeout_ms: None,
            },
        )
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            log_import(state, format!("Transfer review failed: {error}"));
            return Ok(handled);
        }
    };
    let review = match response
        .structured
        .and_then(|value| serde_json::from_value::<InitialTransferReview>(value).ok())
    {
        Some(review) => review,
        None => {
            log_import(state, "Transfer review returned invalid structured output");
            return Ok(handled);
        }
    };
    let rejected = review.not_transfers.into_iter().collect::<HashSet<_>>();
    let mut used_sources = HashSet::new();
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    for (index, (left, right)) in pairs.into_iter().enumerate() {
        let alias = format!("p{index}");
        if rejected.contains(&alias)
            || used_sources.contains(&left.source.id)
            || used_sources.contains(&right.source.id)
        {
            continue;
        }
        let (canonical, duplicate) = if left.source.id < right.source.id {
            (left.source.id, right.source.id)
        } else {
            (right.source.id, left.source.id)
        };
        if database.reconcile_transfer(&canonical, &duplicate)? {
            clear_potential_transfer_note(&mut database, &canonical)?;
            database.append_transaction_logs(&[
                (
                    canonical.clone(),
                    format!("Transfer: reconciled with {duplicate}"),
                ),
                (
                    duplicate.clone(),
                    format!("Transfer: reconciled with {canonical}"),
                ),
            ])?;
            used_sources.insert(canonical.clone());
            used_sources.insert(duplicate.clone());
            handled += 2;
            log_import(
                state,
                format!("Reconciled transfer {alias}: {canonical} / {duplicate}"),
            );
        }
    }
    log_import(
        state,
        format!(
            "Transfer review complete: {} rejected, {} sources reconciled",
            rejected.len(),
            used_sources.len()
        ),
    );
    Ok(handled)
}

fn csv_row(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|value| {
            if value
                .chars()
                .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
            {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                (*value).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn decimal_amount(cents: i64) -> String {
    let cents = i128::from(cents);
    let sign = if cents < 0 { "-" } else { "" };
    let absolute = cents.abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

fn assign_counterpart(
    database: &mut Database,
    source_id: &str,
    account_id: &str,
) -> Result<bool, DatabaseError> {
    let bundle = database.get_source(source_id)?;
    if bundle.projection.postings.len() != 1 || bundle.projection.unallocated_amount == 0 {
        return Ok(false);
    }
    if bundle.source.transaction.account_id.as_deref() == Some(account_id) {
        return Ok(false);
    }
    if !database
        .list_accounts()?
        .iter()
        .any(|account| account.id == account_id)
    {
        return Ok(false);
    }
    let posting_id = uuid::Uuid::now_v7().to_string();
    database.append_changes(
        source_id,
        vec![
            Change::Split {
                target_id: posting_id.clone(),
            },
            Change::Set {
                target_id: posting_id.clone(),
                field: "account_id".into(),
                value: json!(account_id),
            },
            Change::Set {
                target_id: posting_id,
                field: "amount".into(),
                value: json!(bundle.projection.unallocated_amount),
            },
        ],
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn payee_inference_uses_stable_ids_and_local_potentials() {
        let batch = vec![
            PendingPayeeResolution {
                source_ids: vec!["source-1".into(), "source-1-copy".into()],
                imported_payee: "POS  PURCHASE\nAMAZON PRIME #1234".into(),
                potential_payee: Some("Amazon".into()),
                amount_direction: "outflow".into(),
                local_reason: "below threshold".into(),
            },
            PendingPayeeResolution {
                source_ids: vec!["source-2".into()],
                imported_payee: "NEW MERCHANT 9912".into(),
                potential_payee: None,
                amount_direction: "inflow".into(),
                local_reason: "no candidates".into(),
            },
        ];

        let input = payee_inference_input(&batch, &[]);
        assert!(input.prompt.contains(
            "p1: POS PURCHASE AMAZON PRIME #1234 (potential: Amazon; direction: outflow)"
        ));
        assert!(
            input
                .prompt
                .contains("p2: NEW MERCHANT 9912 (direction: inflow)")
        );
        assert!(!input.prompt.contains("potential: none"));
        assert!(
            input
                .prompt
                .contains("transfer: Transfer between financial accounts")
        );
        let schema = input.output_schema.unwrap();
        assert_eq!(schema["required"], json!(["p1", "p2"]));
        assert_eq!(
            schema["properties"]["p1"],
            json!({ "$ref": "#/$defs/result" })
        );
        assert_eq!(
            schema["properties"]["p2"],
            json!({ "$ref": "#/$defs/result" })
        );
        assert_eq!(
            schema["$defs"]["result"]["required"],
            json!(["payee", "category"])
        );

        assert_eq!(
            parse_inferred_payees(
                &json!({
                    "p2": { "payee": "New Merchant", "category": "transfer" },
                    "p1": { "payee": "Amazon Prime", "category": null }
                }),
                &batch,
                &[],
            ),
            Some(vec![
                InferredPayee {
                    source_id: "source-1".into(),
                    imported_payee: "POS  PURCHASE\nAMAZON PRIME #1234".into(),
                    payee: "Amazon Prime".into(),
                    local_reason: "below threshold".into(),
                    resolution_source: "LLM".into(),
                    category_account_id: None,
                    potential_transfer: false,
                },
                InferredPayee {
                    source_id: "source-1-copy".into(),
                    imported_payee: "POS  PURCHASE\nAMAZON PRIME #1234".into(),
                    payee: "Amazon Prime".into(),
                    local_reason: "below threshold".into(),
                    resolution_source: "LLM".into(),
                    category_account_id: None,
                    potential_transfer: false,
                },
                InferredPayee {
                    source_id: "source-2".into(),
                    imported_payee: "NEW MERCHANT 9912".into(),
                    payee: "New Merchant".into(),
                    local_reason: "no candidates".into(),
                    resolution_source: "LLM".into(),
                    category_account_id: None,
                    potential_transfer: true,
                },
            ])
        );
        assert_eq!(
            parse_inferred_payees(
                &json!({ "p1": { "payee": "Amazon Prime", "category": null } }),
                &batch,
                &[],
            ),
            None
        );
        assert_eq!(
            parse_inferred_payees(
                &json!({
                    "p1": { "payee": "Amazon Prime", "category": null },
                    "p2": { "payee": "New Merchant", "category": null },
                    "p3": { "payee": "Wrong", "category": null }
                }),
                &batch,
                &[],
            ),
            None
        );
    }

    #[test]
    fn payee_inference_can_return_a_confident_category() {
        let batch = vec![PendingPayeeResolution {
            source_ids: vec!["source-1".into()],
            imported_payee: "WHOLEFDS #10234".into(),
            potential_payee: Some("Whole Foods".into()),
            amount_direction: "outflow".into(),
            local_reason: "below threshold".into(),
        }];
        let categories = vec![PayeeCategory {
            alias: "c1".into(),
            account_id: "groceries".into(),
            label: "expense: Expenses > Groceries".into(),
            account_type: CategoryAccountType::Expense,
        }];

        let input = payee_inference_input(&batch, &categories);
        assert!(input.prompt.contains("c1: expense: Expenses > Groceries"));
        assert_eq!(
            parse_inferred_payees(
                &json!({ "p1": { "payee": "sq *WHOLE FOODS", "category": "c1" } }),
                &batch,
                &categories,
            ),
            Some(vec![InferredPayee {
                source_id: "source-1".into(),
                imported_payee: "WHOLEFDS #10234".into(),
                payee: "Whole Foods".into(),
                local_reason: "below threshold".into(),
                resolution_source: "LLM".into(),
                category_account_id: Some("groceries".into()),
                potential_transfer: false,
            }])
        );
        assert_eq!(
            parse_inferred_payees(
                &json!({ "p1": { "payee": "Whole Foods", "category": "c2" } }),
                &batch,
                &categories,
            ),
            None
        );
    }

    #[test]
    fn payee_categories_exclude_only_top_level_groups() {
        let account = |id: &str,
                       parent_id: Option<&str>,
                       name: &str,
                       account_type: AccountType,
                       has_children: bool| AccountRecord {
            id: id.into(),
            parent_id: parent_id.map(str::to_owned),
            external_id: None,
            name: name.into(),
            imported_name: None,
            account_type,
            balance: 0,
            has_children,
        };
        let categories = payee_categories(&[
            account("expenses", None, "Expenses", AccountType::Expense, true),
            account(
                "groceries",
                Some("expenses"),
                "Groceries",
                AccountType::Expense,
                false,
            ),
            account("giving", None, "Giving", AccountType::Expense, false),
            account("income", None, "Income", AccountType::Income, true),
            account(
                "salary",
                Some("income"),
                "Salary",
                AccountType::Income,
                false,
            ),
        ]);

        assert_eq!(
            categories
                .iter()
                .map(|category| category.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "expense: Expenses > Groceries",
                "expense: Giving",
                "income: Income > Salary",
            ]
        );
    }

    #[test]
    fn imported_transactions_use_deterministic_global_batches() {
        let account = |id: &str| AccountRecord {
            id: id.into(),
            parent_id: None,
            external_id: Some(id.into()),
            name: id.into(),
            imported_name: Some(id.into()),
            account_type: AccountType::Asset,
            balance: 0,
            has_children: false,
        };
        let snapshots = vec![
            ImportedAccountSnapshot {
                account: account("b-account"),
                pending_ids: Vec::new(),
            },
            ImportedAccountSnapshot {
                account: account("a-account"),
                pending_ids: Vec::new(),
            },
        ];
        let mut transactions = (0..201)
            .map(|index| QueuedImportedTransaction {
                account_index: index % 2,
                transaction: ImportedTransaction {
                    id: format!("tx-{index:03}"),
                    replaces_pending_id: None,
                    date: "2026-08-06".into(),
                    payee: Some("PAYEE".into()),
                    amount: -100,
                    category: None,
                    pending: false,
                    opening_balance: false,
                    notes: None,
                    raw: Value::Null,
                },
            })
            .collect::<Vec<_>>();

        sort_queued_transactions(&mut transactions, &snapshots);

        assert!(
            transactions[..100]
                .iter()
                .all(|transaction| transaction.account_index == 1)
        );
        assert!(
            transactions[100..]
                .iter()
                .all(|transaction| transaction.account_index == 0)
        );
        assert_eq!(
            transactions
                .chunks(DEFAULT_TRANSACTION_BATCH_SIZE)
                .map(<[QueuedImportedTransaction]>::len)
                .collect::<Vec<_>>(),
            vec![200, 1]
        );
    }

    #[test]
    fn posted_transactions_remove_their_pending_versions_before_batching() {
        let mut transactions = vec![
            ImportedTransaction {
                id: "pending-1".into(),
                replaces_pending_id: None,
                date: "2026-08-06".into(),
                payee: Some("PENDING PAYEE".into()),
                amount: -100,
                category: None,
                pending: true,
                opening_balance: false,
                notes: None,
                raw: Value::Null,
            },
            ImportedTransaction {
                id: "posted-1".into(),
                replaces_pending_id: Some("pending-1".into()),
                date: "2026-08-06".into(),
                payee: Some("POSTED PAYEE".into()),
                amount: -100,
                category: None,
                pending: false,
                opening_balance: false,
                notes: None,
                raw: Value::Null,
            },
        ];

        assert_eq!(remove_superseded_pending(&mut transactions), 1);
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].id, "posted-1");
    }

    #[test]
    fn pending_counterpart_assignments_become_categorization_history() {
        use crate::model::{ImportedAccount, ImportedTransaction, ImportedTransactions};

        let mut database = Database::open(":memory:").unwrap();
        let source_account = database
            .sync_accounts(&[ImportedAccount {
                id: "bank".into(),
                parent_id: None,
                name: "Checking".into(),
                account_type: AccountType::Asset,
            }])
            .unwrap()
            .remove(0)
            .0;
        let category_id = "019fd900-0000-7000-8000-000000000003";
        database
            .upsert_account(
                category_id,
                AccountInput {
                    parent_id: None,
                    name: "Food".into(),
                    account_type: AccountType::Expense,
                },
            )
            .unwrap();
        let transaction = ImportedTransaction {
            id: "whole-foods-1".into(),
            replaces_pending_id: None,
            date: "2026-08-06".into(),
            payee: Some("WHOLEFDS #10234".into()),
            amount: -500,
            category: Some("groceries".into()),
            pending: true,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let result = database
            .sync_transactions(
                &source_account,
                ImportedTransactions {
                    transactions: vec![transaction],
                    pending_ids: vec!["whole-foods-1".into()],
                },
                None,
            )
            .unwrap();
        let source_id = &result.new_source_ids[0];
        database
            .append_payee_resolutions(&[(source_id.clone(), "Whole Foods".into())])
            .unwrap();
        assert!(assign_counterpart(&mut database, source_id, category_id).unwrap());

        let examples = database.categorization_examples().unwrap();
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].payee, "Whole Foods");
        assert_eq!(examples[0].imported_category.as_deref(), Some("groceries"));
        assert_eq!(examples[0].date, "2026-08-06");
        assert_eq!(examples[0].source_account_id, source_account.id);
        assert_eq!(examples[0].counterpart_account_id, category_id);
        assert_eq!(
            database
                .categorization_query(source_id)
                .unwrap()
                .unwrap()
                .payee,
            "Whole Foods"
        );
    }

    #[test]
    fn transfer_candidates_are_scoped_to_touched_sources_not_new_accounts() {
        use crate::model::{ImportedAccount, ImportedTransaction, ImportedTransactions};

        let mut database = Database::open(":memory:").unwrap();
        let accounts = database
            .sync_accounts(&[
                ImportedAccount {
                    id: "checking".into(),
                    parent_id: None,
                    name: "Checking".into(),
                    account_type: AccountType::Asset,
                },
                ImportedAccount {
                    id: "savings".into(),
                    parent_id: None,
                    name: "Savings".into(),
                    account_type: AccountType::Asset,
                },
            ])
            .unwrap();
        let left = ImportedTransaction {
            id: "left".into(),
            replaces_pending_id: None,
            date: "2026-08-06".into(),
            payee: Some("Transfer to Savings".into()),
            amount: -5000,
            category: None,
            pending: false,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let right = ImportedTransaction {
            id: "right".into(),
            replaces_pending_id: None,
            date: "2026-08-06".into(),
            payee: Some("Transfer from Checking".into()),
            amount: 5000,
            category: None,
            pending: false,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let result = database
            .sync_transaction_batch(&[(&accounts[0].0, &left), (&accounts[1].0, &right)], None)
            .unwrap();

        let pairs = database
            .transfer_pairs_for_sources(&[result.processed_source_ids[0].clone()])
            .unwrap();
        assert_eq!(pairs.len(), 1);
        assert!(
            pairs[0].0.source.id == result.processed_source_ids[0]
                || pairs[0].1.source.id == result.processed_source_ids[0]
        );
        let left_id = pairs[0].0.source.id.clone();
        let right_id = pairs[0].1.source.id.clone();
        let (canonical, duplicate) = if left_id < right_id {
            (left_id, right_id)
        } else {
            (right_id, left_id)
        };
        assert!(
            mark_potential_transfer(
                &mut database,
                &canonical,
                "Category: Potential transfer (LLM)",
            )
            .unwrap()
        );
        assert!(database.reconcile_transfer(&canonical, &duplicate).unwrap());
        assert!(clear_potential_transfer_note(&mut database, &canonical).unwrap());
        assert!(
            !mark_potential_transfer(
                &mut database,
                &canonical,
                "Category: Potential transfer (LLM)",
            )
            .unwrap()
        );
        assert_ne!(
            database.get_source(&canonical).unwrap().projection.postings[0]
                .posting
                .notes
                .as_deref(),
            Some("Potential transfer")
        );
        let transfer_history = database.categorization_examples().unwrap();
        assert_eq!(transfer_history.len(), 1);
        assert!(transfer_history[0].transfer);
        assert!(transfer_history[0].counterpart_account_id.is_empty());
        let first_page = database
            .list_all_transactions(
                1,
                None,
                Some(&[AccountType::Asset, AccountType::Liability]),
                None,
            )
            .unwrap();
        let second_page = database
            .list_all_transactions(
                1,
                first_page.next.as_ref(),
                Some(&[AccountType::Asset, AccountType::Liability]),
                None,
            )
            .unwrap();
        assert_eq!(first_page.transactions[0].source_id, canonical);
        assert_eq!(second_page.transactions[0].source_id, canonical);
        assert_ne!(
            first_page.transactions[0].posting_id,
            second_page.transactions[0].posting_id
        );
        let all = database
            .list_all_transactions(
                100,
                None,
                Some(&[AccountType::Asset, AccountType::Liability]),
                None,
            )
            .unwrap();
        let transfer_rows = all
            .transactions
            .iter()
            .filter(|transaction| transaction.source_id == canonical)
            .collect::<Vec<_>>();
        assert_eq!(transfer_rows.len(), 2);
        assert_ne!(transfer_rows[0].posting_id, transfer_rows[1].posting_id);
        assert_eq!(transfer_rows[0].amount + transfer_rows[1].amount, 0);
        let possible = ImportedTransaction {
            id: "possible".into(),
            payee: Some("Transfer to Unknown".into()),
            amount: -2500,
            ..left
        };
        let possible = database
            .sync_transactions(
                &accounts[0].0,
                ImportedTransactions {
                    transactions: vec![possible],
                    pending_ids: Vec::new(),
                },
                None,
            )
            .unwrap();
        let possible_id = &possible.new_source_ids[0];
        assert!(
            mark_potential_transfer(
                &mut database,
                possible_id,
                "Category: Potential transfer (test)",
            )
            .unwrap()
        );
        let possible = database.get_source(possible_id).unwrap();
        assert_eq!(
            possible.projection.postings[0].posting.notes.as_deref(),
            Some("Potential transfer")
        );
        let row = database
            .list_account(&accounts[0].0.id, 100, None, None)
            .unwrap()
            .transactions
            .into_iter()
            .find(|transaction| transaction.source_id == *possible_id)
            .unwrap();
        assert!(
            row.logs
                .iter()
                .any(|log| log.message.contains("Potential transfer"))
        );
    }
}
