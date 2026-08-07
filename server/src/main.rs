mod app_log;
mod config;
mod db;
mod model;
mod perf;
mod plugin;

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    config::{ConfigError, ConfigFile},
    db::{Database, DatabaseError},
    model::{
        AccountCursor, AccountDeletion, AccountInput, AccountRecord, AccountTransactionsPage,
        AccountType, Change, ChangesInput, ImportPluginConfig, ImportSummary,
        ImportTransactionsInput, ImportedTransaction, InferenceConfig, InferenceGenerateInput,
        InferenceGenerateOutput, InferenceModelInput, InferenceModels, ModelError, PayeeFormat,
        PluginStatus, PluginsConfigFile, SourceBundle, SourceInput,
    },
    plugin::{ImportManager, InferenceManager, PluginError},
};
use monet_server::{
    categorization::{
        CategorizationMethod, CategorizationPipeline, CategoryAccount, CategoryAccountType,
    },
    payee_resolution::{PayeeResolver, clean_payee_name, normalize_payee},
};

#[derive(Clone)]
struct AppState {
    database: Arc<Mutex<Database>>,
    inference: Arc<InferenceManager>,
    importer: Arc<ImportManager>,
    config: Arc<ConfigFile>,
    import_log: Arc<Mutex<ImportLog>>,
}

#[derive(Serialize)]
struct Health {
    healthy: bool,
}

#[derive(Serialize)]
struct TransactionCount {
    count: usize,
}

#[derive(Clone, Default, Serialize)]
struct ImportLog {
    running: bool,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Deserialize)]
struct AccountPageQuery {
    limit: Option<usize>,
    before_date: Option<String>,
    before_source_id: Option<String>,
    before_posting_id: Option<String>,
    account_types: Option<String>,
    search: Option<String>,
}

#[derive(Deserialize)]
struct DeleteAccountQuery {
    #[serde(default)]
    delete_transactions: bool,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRunInput {
    end_date: Option<String>,
}

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

#[derive(Debug)]
enum ApiError {
    Database(DatabaseError),
    Plugin(PluginError),
    Config(ConfigError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Database(error) => {
                let status = match error {
                    DatabaseError::SourceNotFound(_) | DatabaseError::AccountNotFound(_) => {
                        StatusCode::NOT_FOUND
                    }
                    DatabaseError::Model(_)
                    | DatabaseError::Json(_)
                    | DatabaseError::SyncedAccountDeletion(_)
                    | DatabaseError::SyncedAccountStructure(_)
                    | DatabaseError::CategoryTransactionDeletion => {
                        StatusCode::UNPROCESSABLE_ENTITY
                    }
                    DatabaseError::Sql(_) | DatabaseError::Poisoned => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, error.to_string())
            }
            Self::Plugin(error) => {
                let status = match error {
                    PluginError::NotConfigured => StatusCode::NOT_FOUND,
                    PluginError::InvalidCommand | PluginError::Protocol(_) => {
                        StatusCode::UNPROCESSABLE_ENTITY
                    }
                    PluginError::Timeout => StatusCode::GATEWAY_TIMEOUT,
                    PluginError::Launch(_)
                    | PluginError::Io(_)
                    | PluginError::Json(_)
                    | PluginError::Exited
                    | PluginError::MessageTooLarge
                    | PluginError::Remote { .. } => StatusCode::BAD_GATEWAY,
                };
                (status, error.to_string())
            }
            Self::Config(error) => (StatusCode::UNPROCESSABLE_ENTITY, error.to_string()),
        };
        (status, Json(ErrorBody { error: message })).into_response()
    }
}

impl From<DatabaseError> for ApiError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<PluginError> for ApiError {
    fn from(error: PluginError) -> Self {
        Self::Plugin(error)
    }
}

impl From<ConfigError> for ApiError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = env::var("MONET_DB_PATH").unwrap_or_else(|_| ".money-tui/money.db".into());
    if let Some(parent) = Path::new(&database_path).parent() {
        fs::create_dir_all(parent)?;
    }
    let address: SocketAddr = env::var("MONET_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:47832".into())
        .parse()?;
    let app = create_router(Database::open(&database_path)?, ConfigFile::open()?);

    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("monet server listening on http://{address}");
    app_log::write("server", &format!("listening on http://{address}"));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn create_router(database: Database, config: ConfigFile) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/import/run", post(run_import))
        .route("/import/logs", get(import_logs))
        .route("/import/status", get(import_status))
        .route("/import/restart", post(import_restart))
        .route("/plugins", get(get_plugins))
        .route("/inference/status", get(inference_status))
        .route("/inference/models", get(inference_models))
        .route("/inference/model", put(update_inference_model))
        .route("/inference/generate", post(inference_generate))
        .route("/inference/restart", post(inference_restart))
        .route("/accounts", get(list_accounts))
        .route("/transactions", get(list_transactions))
        .route(
            "/transactions/unbalanced",
            get(list_unbalanced_transactions),
        )
        .route(
            "/transactions/unbalanced/count",
            get(count_unbalanced_transactions),
        )
        .route(
            "/accounts/{account_id}",
            put(upsert_account).delete(delete_account),
        )
        .route(
            "/accounts/{account_id}/transactions",
            get(list_account_transactions),
        )
        .route(
            "/sources/{source_id}",
            put(upsert_source)
                .get(get_source)
                .delete(delete_pending_source),
        )
        .route("/sources/{source_id}/changes", post(append_changes))
        .layer(middleware::from_fn(performance_middleware))
        .with_state(AppState {
            database: Arc::new(Mutex::new(database)),
            inference: Arc::new(InferenceManager::default()),
            importer: Arc::new(ImportManager::default()),
            config: Arc::new(config),
            import_log: Arc::new(Mutex::new(ImportLog::default())),
        })
}

async fn performance_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let response = next.run(request).await;
    crate::perf::log(
        "http.request",
        serde_json::json!({
            "method": method,
            "path": path,
            "status": response.status().as_u16(),
            "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
        }),
    );
    response
}

async fn health() -> Json<Health> {
    Json(Health { healthy: true })
}

async fn get_plugins(State(state): State<AppState>) -> Result<Json<PluginsConfigFile>, ApiError> {
    let config = state.config.load()?;
    Ok(Json(PluginsConfigFile {
        path: state.config.path(),
        inference: config.inference,
        import: config.import,
    }))
}

fn require_inference_config(state: &AppState) -> Result<InferenceConfig, ApiError> {
    state
        .config
        .load()?
        .inference
        .ok_or_else(|| PluginError::NotConfigured.into())
}

fn require_import_config(state: &AppState) -> Result<ImportPluginConfig, ApiError> {
    state
        .config
        .load()?
        .import
        .ok_or_else(|| PluginError::NotConfigured.into())
}

async fn inference_status(State(state): State<AppState>) -> Result<Json<PluginStatus>, ApiError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.status(config).await?))
}

async fn inference_models(
    State(state): State<AppState>,
) -> Result<Json<InferenceModels>, ApiError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.models(config).await?))
}

async fn update_inference_model(
    State(state): State<AppState>,
    Json(input): Json<InferenceModelInput>,
) -> Result<Json<InferenceConfig>, ApiError> {
    Ok(Json(state.config.set_inference_model(input.model)?))
}

async fn inference_generate(
    State(state): State<AppState>,
    Json(input): Json<InferenceGenerateInput>,
) -> Result<Json<InferenceGenerateOutput>, ApiError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.generate(config, input).await?))
}

async fn inference_restart(State(state): State<AppState>) -> Json<Health> {
    state.inference.restart().await;
    Json(Health { healthy: true })
}

async fn import_status(State(state): State<AppState>) -> Result<Json<PluginStatus>, ApiError> {
    let config = require_import_config(&state)?;
    Ok(Json(state.importer.status(config).await?))
}

async fn import_restart(State(state): State<AppState>) -> Json<Health> {
    state.importer.restart().await;
    Json(Health { healthy: true })
}

async fn run_import(
    State(state): State<AppState>,
    input: Option<Json<ImportRunInput>>,
) -> Result<Json<ImportSummary>, ApiError> {
    if let Ok(mut log) = state.import_log.lock() {
        log.running = true;
        log.lines.clear();
        log.lines.push("Starting transaction sync".into());
    }
    app_log::write("sync", "Starting transaction sync");
    let result = run_import_inner(&state, input.and_then(|input| input.end_date.clone())).await;
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
    result.map(Json)
}

async fn import_logs(State(state): State<AppState>) -> Json<ImportLog> {
    Json(
        state
            .import_log
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default(),
    )
}

fn log_import(state: &AppState, message: impl Into<String>) {
    let message = message.into();
    app_log::write("sync", &message);
    if let Ok(mut log) = state.import_log.lock() {
        log.lines.push(message);
    }
}

async fn run_import_inner(
    state: &AppState,
    end_date: Option<String>,
) -> Result<ImportSummary, ApiError> {
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
    state: &AppState,
    unresolved: &[PendingPayeeResolution],
    categories: &[PayeeCategory],
) -> Result<(usize, HashMap<String, String>, HashSet<String>), ApiError> {
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
    state: &AppState,
    inferred: Vec<InferredPayee>,
) -> Result<(usize, HashMap<String, String>, HashSet<String>), ApiError> {
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
    let mut properties = serde_json::Map::new();
    let mut required = Vec::with_capacity(batch.len());
    let category_values = [serde_json::Value::Null, json!("transfer")]
        .into_iter()
        .chain(categories.iter().map(|category| json!(category.alias)))
        .collect::<Vec<_>>();
    let result_schema = json!({
        "type": "object",
        "properties": {
            "payee": { "type": "string", "minLength": 1 },
            "category": {
                "type": ["string", "null"],
                "enum": category_values,
            },
        },
        "required": ["payee", "category"],
        "additionalProperties": false,
    });
    for index in 0..batch.len() {
        let id = format!("p{}", index + 1);
        properties.insert(id.clone(), result_schema.clone());
        required.push(id.clone());
    }
    let category_prompt = format!(
        "CATEGORIES\ntransfer: Transfer between financial accounts{}\n\n",
        categories
            .iter()
            .map(|category| format!("\n{}: {}", category.alias, category.label))
            .collect::<String>()
    );
    let category_instruction = if categories.is_empty() {
        " Return category \"transfer\" only when the transaction clearly looks like movement between financial accounts; otherwise return null."
    } else {
        " Return category \"transfer\" when the transaction clearly looks like movement between financial accounts. Otherwise return one category ID from the provided CATEGORIES only when confident; return null when uncertain."
    };
    InferenceGenerateInput {
        model: None,
        system: Some(format!(
            "Clean raw imported transaction payees. Each input has a stable ID, the full messy payee, its amount direction, and may have the top local historical potential in parentheses. Inflow means a positive amount and outflow means a negative amount; direction does not by itself imply income or expense because transactions may be transfers, refunds, or payments. The potential may be wrong: reuse it only when it represents the same real payee. Otherwise produce a concise new cleaned payee. Preserve meaningful qualifiers and distinct services or sub-brands. Avoid numbers unless they are an important part of the payee's identity; omit dates, store IDs, transaction references, and other incidental numbers.{category_instruction} Return exactly one result for every input ID, keep every ID unchanged, and return no other IDs."
        )),
        prompt: format!(
            "{category_prompt}UNRESOLVED PAYEES\n{}",
            payee_inference_lines(batch).join("\n")
        ),
        output_schema: Some(json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })),
        timeout_ms: None,
    }
}

fn payee_inference_lines(batch: &[PendingPayeeResolution]) -> Vec<String> {
    batch
        .iter()
        .enumerate()
        .map(|(index, pending)| {
            let details = pending
                .potential_payee
                .as_deref()
                .map(|payee| {
                    format!(
                        "potential: {}; direction: {}",
                        prompt_line(payee),
                        pending.amount_direction
                    )
                })
                .unwrap_or_else(|| format!("direction: {}", pending.amount_direction));
            format!(
                "p{}: {} ({details})",
                index + 1,
                prompt_line(&pending.imported_payee)
            )
        })
        .collect()
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
    state: &AppState,
    pipeline: &CategorizationPipeline,
    source_ids: &[String],
    suggested_categories: &HashMap<String, String>,
    potential_transfers: &HashSet<String>,
    new_source_ids: &HashSet<String>,
) -> Result<usize, ApiError> {
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
    state: &AppState,
    processed_source_ids: &[String],
) -> Result<usize, ApiError> {
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

async fn upsert_account(
    State(state): State<AppState>,
    AxumPath(account_id): AxumPath<String>,
    Json(input): Json<AccountInput>,
) -> Result<Json<AccountRecord>, ApiError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.upsert_account(&account_id, input)?))
}

async fn list_accounts(
    State(state): State<AppState>,
) -> Result<Json<Vec<AccountRecord>>, ApiError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_accounts()?))
}

async fn delete_account(
    State(state): State<AppState>,
    AxumPath(account_id): AxumPath<String>,
    Query(query): Query<DeleteAccountQuery>,
) -> Result<Json<AccountDeletion>, ApiError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(
        database.delete_account(&account_id, query.delete_transactions)?,
    ))
}

async fn upsert_source(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
    Json(input): Json<SourceInput>,
) -> Result<Json<SourceBundle>, ApiError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.upsert_source(&source_id, input)?))
}

async fn get_source(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> Result<Json<SourceBundle>, ApiError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.get_source(&source_id)?))
}

async fn delete_pending_source(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    database.delete_pending_source(&source_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn append_changes(
    State(state): State<AppState>,
    AxumPath(source_id): AxumPath<String>,
    Json(input): Json<ChangesInput>,
) -> Result<Json<SourceBundle>, ApiError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.append_changes(&source_id, input.changes)?))
}

async fn list_account_transactions(
    State(state): State<AppState>,
    AxumPath(account_id): AxumPath<String>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, ApiError> {
    let (limit, before, _, search) = transaction_page(query)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_account(
        &account_id,
        limit,
        before.as_ref(),
        search.as_deref(),
    )?))
}

async fn list_transactions(
    State(state): State<AppState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, ApiError> {
    let (limit, before, account_types, search) = transaction_page(query)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_all_transactions(
        limit,
        before.as_ref(),
        account_types.as_deref(),
        search.as_deref(),
    )?))
}

async fn list_unbalanced_transactions(
    State(state): State<AppState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, ApiError> {
    let (limit, before, account_types, search) = transaction_page(query)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_unbalanced_transactions(
        limit,
        before.as_ref(),
        account_types.as_deref(),
        search.as_deref(),
    )?))
}

async fn count_unbalanced_transactions(
    State(state): State<AppState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<TransactionCount>, ApiError> {
    let (_, _, account_types, search) = transaction_page(query)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(TransactionCount {
        count: database
            .count_unbalanced_transactions(account_types.as_deref(), search.as_deref())?,
    }))
}

fn transaction_page(
    query: AccountPageQuery,
) -> Result<
    (
        usize,
        Option<AccountCursor>,
        Option<Vec<AccountType>>,
        Option<String>,
    ),
    ApiError,
> {
    let before = match (
        query.before_date,
        query.before_source_id,
        query.before_posting_id,
    ) {
        (Some(date), Some(source_id), Some(posting_id)) => Some(AccountCursor {
            date,
            source_id,
            posting_id,
        }),
        (None, None, None) => None,
        _ => {
            return Err(DatabaseError::Model(ModelError::InvalidValue {
                field: "before".into(),
                expected: "before_date, before_source_id, and before_posting_id",
            })
            .into());
        }
    };
    let account_types = query
        .account_types
        .map(|types| {
            types
                .split(',')
                .map(AccountType::parse)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(DatabaseError::from)?;
    Ok((
        query.limit.unwrap_or(50).clamp(1, 100),
        before,
        account_types,
        query.search,
    ))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    const SOURCE_ID: &str = "019bf123-0000-7000-8000-000000000001";
    const CHECKING_ID: &str = "019bf123-0000-7000-8000-000000000010";
    const EXPENSES_ID: &str = "019bf123-0000-7000-8000-000000000011";
    const GROCERIES_ID: &str = "019bf123-0000-7000-8000-000000000012";
    const HOUSEHOLD_ID: &str = "019bf123-0000-7000-8000-000000000013";
    const PRODUCE_ID: &str = "019bf123-0000-7000-8000-000000000014";
    const CASH_ID: &str = "019bf123-0000-7000-8000-000000000015";
    const SAVINGS_ID: &str = "019bf123-0000-7000-8000-000000000016";
    const SECOND_SOURCE_ID: &str = "019bf123-0000-7000-8000-000000000017";
    const TRANSFER_POSTING_ID: &str = "019bf123-0000-7000-8000-000000000018";
    const CARD_ID: &str = "019bf123-0000-7000-8000-000000000019";
    const THIRD_SOURCE_ID: &str = "019bf123-0000-7000-8000-000000000020";
    const GROCERIES_POSTING_ID: &str = "019bf123-0000-7000-8000-000000000002";
    const HOUSEHOLD_POSTING_ID: &str = "019bf123-0000-7000-8000-000000000003";

    fn test_app() -> Router {
        let path =
            std::env::temp_dir().join(format!("monet-test-config-{}.json", std::process::id()));
        create_router(
            Database::open(":memory:").unwrap(),
            ConfigFile::at(path).unwrap(),
        )
    }

    fn source(amount: i64, payee: &str) -> Value {
        json!({
            "source": "test-bank",
            "external_account_id": "checking-123",
            "external_transaction_id": "purchase-1",
            "account_id": CHECKING_ID,
            "date": "2026-07-20",
            "payee": payee,
            "amount": amount,
            "category": "Shopping",
            "pending": false,
            "opening_balance": false,
            "notes": null,
            "raw": { "description": payee }
        })
    }

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
        assert_eq!(
            input.output_schema.unwrap()["required"],
            json!(["p1", "p2"])
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

    async fn request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let response = app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    async fn put_account(
        app: &Router,
        id: &str,
        parent_id: Option<&str>,
        name: &str,
        account_type: &str,
    ) -> Value {
        let (status, body) = request(
            app,
            Method::PUT,
            &format!("/accounts/{id}"),
            Some(json!({
                "parent_id": parent_id,
                "name": name,
                "type": account_type
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn setup_accounts(app: &Router) {
        put_account(app, CHECKING_ID, None, "Checking", "asset").await;
        put_account(app, EXPENSES_ID, None, "Expenses", "expense").await;
        put_account(app, GROCERIES_ID, Some(EXPENSES_ID), "Groceries", "expense").await;
        put_account(app, HOUSEHOLD_ID, Some(EXPENSES_ID), "Household", "expense").await;
    }

    async fn put_source(app: &Router, amount: i64, payee: &str) -> Value {
        let (status, body) = request(
            app,
            Method::PUT,
            &format!("/sources/{SOURCE_ID}"),
            Some(source(amount, payee)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    fn split_changes(groceries_amount: i64, household_amount: Option<i64>) -> Value {
        let mut changes = vec![
            json!({ "operation": "split", "target_id": GROCERIES_POSTING_ID }),
            json!({
                "operation": "set",
                "target_id": GROCERIES_POSTING_ID,
                "field": "account_id",
                "value": GROCERIES_ID
            }),
            json!({
                "operation": "set",
                "target_id": GROCERIES_POSTING_ID,
                "field": "amount",
                "value": groceries_amount
            }),
        ];
        if let Some(amount) = household_amount {
            changes.extend([
                json!({ "operation": "split", "target_id": HOUSEHOLD_POSTING_ID }),
                json!({
                    "operation": "set",
                    "target_id": HOUSEHOLD_POSTING_ID,
                    "field": "account_id",
                    "value": HOUSEHOLD_ID
                }),
                json!({
                    "operation": "set",
                    "target_id": HOUSEHOLD_POSTING_ID,
                    "field": "amount",
                    "value": amount
                }),
            ]);
        }
        json!({ "changes": changes })
    }

    #[test]
    fn existing_synced_account_names_are_backfilled() {
        let path = std::env::temp_dir().join(format!("monet-{}.db", uuid::Uuid::now_v7()));
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE accounts (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT REFERENCES accounts(id),
                    external_id TEXT UNIQUE,
                    name TEXT NOT NULL,
                    type TEXT NOT NULL,
                    latest_external_transaction_id TEXT
                 );
                 INSERT INTO accounts (id, external_id, name, type)
                 VALUES ('{CHECKING_ID}', 'bank', 'Checking', 'asset');
                 INSERT INTO accounts (id, name, type)
                 VALUES ('{EXPENSES_ID}', 'Expenses', 'expense');"
            ))
            .unwrap();
        drop(connection);

        let database = Database::open(path.to_str().unwrap()).unwrap();
        let accounts = database.list_accounts().unwrap();
        let checking = accounts
            .iter()
            .find(|account| account.id == CHECKING_ID)
            .unwrap();
        let expenses = accounts
            .iter()
            .find(|account| account.id == EXPENSES_ID)
            .unwrap();
        assert_eq!(checking.imported_name.as_deref(), Some("Checking"));
        assert_eq!(expenses.imported_name, None);
        drop(database);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn accounts_support_types_and_hierarchy() {
        let app = test_app();
        setup_accounts(&app).await;

        let (status, accounts) = request(&app, Method::GET, "/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(accounts.as_array().unwrap().len(), 4);
        let groceries = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["id"] == GROCERIES_ID)
            .unwrap();
        assert_eq!(groceries["parent_id"], EXPENSES_ID);
        assert_eq!(groceries["type"], "expense");
        assert_eq!(groceries["has_children"], false);
        let expenses = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["id"] == EXPENSES_ID)
            .unwrap();
        assert_eq!(expenses["has_children"], true);
        assert!(
            accounts
                .as_array()
                .unwrap()
                .iter()
                .all(|account| account.get("currency").is_none())
        );

        let (status, _) = request(
            &app,
            Method::PUT,
            &format!("/accounts/{EXPENSES_ID}"),
            Some(json!({
                "parent_id": GROCERIES_ID,
                "name": "Expenses",
                "type": "expense"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let (status, _) = request(
            &app,
            Method::PUT,
            &format!("/accounts/{CHECKING_ID}"),
            Some(json!({
                "parent_id": EXPENSES_ID,
                "name": "Checking",
                "type": "asset"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn accounts_can_be_renamed_reparented_and_retyped_as_roots() {
        let app = test_app();
        setup_accounts(&app).await;

        let moved = put_account(&app, HOUSEHOLD_ID, Some(GROCERIES_ID), "Home", "expense").await;
        assert_eq!(moved["name"], "Home");
        assert_eq!(moved["parent_id"], GROCERIES_ID);
        assert_eq!(moved["type"], "expense");

        let root = put_account(&app, HOUSEHOLD_ID, None, "Home", "income").await;
        assert_eq!(root["name"], "Home");
        assert_eq!(root["parent_id"], Value::Null);
        assert_eq!(root["type"], "income");
    }

    #[tokio::test]
    async fn virtual_registers_filter_by_account_type() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;

        let (status, asset_count) = request(
            &app,
            Method::GET,
            "/transactions/unbalanced/count?account_types=asset,liability,equity",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(asset_count["count"], 1);
        let (status, expense_count) = request(
            &app,
            Method::GET,
            "/transactions/unbalanced/count?account_types=income,expense",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(expense_count["count"], 0);

        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(json!({
                "changes": [
                    {
                        "operation": "set",
                        "target_id": SOURCE_ID,
                        "field": "payee",
                        "value": "Renamed"
                    },
                    {
                        "operation": "set",
                        "target_id": SOURCE_ID,
                        "field": "notes",
                        "value": "Birthday trip"
                    }
                ]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        for search in ["target", "renamed", "birthday%20trip", "100.00"] {
            let (status, matches) = request(
                &app,
                Method::GET,
                &format!("/transactions?account_types=asset,liability,equity&search={search}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(matches["transactions"].as_array().unwrap().len(), 1);
        }
        let (status, no_matches) = request(
            &app,
            Method::GET,
            "/transactions?account_types=asset,liability,equity&search=payroll",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(no_matches["transactions"].as_array().unwrap().is_empty());

        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(10_000, None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, assets) = request(
            &app,
            Method::GET,
            "/transactions?account_types=asset,liability,equity",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(assets["transactions"][0]["account_id"], CHECKING_ID);
        assert_eq!(assets["transactions"][0]["amount"], -10_000);

        let (status, expenses) = request(
            &app,
            Method::GET,
            "/transactions?account_types=income,expense",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(expenses["transactions"][0]["account_id"], GROCERIES_ID);
        assert_eq!(expenses["transactions"][0]["amount"], 10_000);
    }

    #[tokio::test]
    async fn an_unassigned_source_materializes_one_posting() {
        let app = test_app();
        setup_accounts(&app).await;
        let body = put_source(&app, -10_000, "TARGET #1842").await;

        assert_eq!(body["postings"].as_array().unwrap().len(), 1);
        assert_eq!(body["postings"][0]["id"], SOURCE_ID);
        assert_eq!(body["postings"][0]["account_id"], CHECKING_ID);
        assert_eq!(body["postings"][0]["amount"], -10_000);
        assert_eq!(body["unallocated_amount"], 10_000);
        assert_eq!(body["source"]["category"], "Shopping");

        let (status, count) =
            request(&app, Method::GET, "/transactions/unbalanced/count", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(count["count"], 1);
    }

    #[tokio::test]
    async fn source_changes_survive_reimport_and_rebuild_every_posting() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "TARGET #1842").await;

        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(json!({
                "changes": [{
                    "operation": "set",
                    "target_id": SOURCE_ID,
                    "field": "payee",
                    "value": "Target"
                }]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let body = put_source(&app, -12_000, "TARGET STORE 1842").await;
        assert_eq!(body["source"]["payee"], "TARGET STORE 1842");
        assert_eq!(body["postings"][0]["payee"], "Target");
        assert_eq!(body["postings"][0]["amount"], -12_000);
        assert_eq!(body["unallocated_amount"], 12_000);
    }

    #[tokio::test]
    async fn split_postings_expose_unallocated_amount_and_account_registers() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;

        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(6000, Some(3000))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["postings"].as_array().unwrap().len(), 3);
        assert_eq!(body["unallocated_amount"], 1000);

        let body = put_source(&app, -12_000, "Target after import").await;
        assert_eq!(body["unallocated_amount"], 3000);
        assert!(
            body["postings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|posting| posting["payee"] == "Target after import")
        );

        let (status, checking) = request(
            &app,
            Method::GET,
            &format!("/accounts/{CHECKING_ID}/transactions"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(checking["transactions"][0]["amount"], -12_000);
        assert_eq!(checking["transactions"][0]["posting_count"], 3);
        assert_eq!(checking["transactions"][0]["unallocated_amount"], 3000);
        assert_eq!(
            checking["transactions"][0]["other_postings"][0]["account_name"],
            "Groceries"
        );
        assert_eq!(
            checking["transactions"][0]["other_postings"][0]["posting_id"],
            GROCERIES_POSTING_ID
        );
        assert_eq!(
            checking["transactions"][0]["other_postings"][1]["account_name"],
            "Household"
        );

        let (status, groceries) = request(
            &app,
            Method::GET,
            &format!("/accounts/{GROCERIES_ID}/transactions"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(groceries["transactions"][0]["amount"], 6000);

        let (status, accounts) = request(&app, Method::GET, "/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        let balance = |id: &str| {
            accounts
                .as_array()
                .unwrap()
                .iter()
                .find(|account| account["id"] == id)
                .unwrap()["balance"]
                .as_i64()
                .unwrap()
        };
        assert_eq!(balance(CHECKING_ID), -12_000);
        assert_eq!(balance(EXPENSES_ID), 9000);
        assert_eq!(balance(GROCERIES_ID), 6000);
        assert_eq!(balance(HOUSEHOLD_ID), 3000);

        let (status, child) = request(
            &app,
            Method::PUT,
            &format!("/accounts/{PRODUCE_ID}"),
            Some(json!({
                "parent_id": GROCERIES_ID,
                "name": "Produce",
                "type": "expense"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(child["parent_id"], GROCERIES_ID);

        let (status, accounts) = request(&app, Method::GET, "/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        let groceries = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["id"] == GROCERIES_ID)
            .unwrap();
        assert_eq!(groceries["has_children"], true);
        assert_eq!(groceries["balance"], 6000);
    }

    #[tokio::test]
    async fn parent_account_register_includes_each_descendant_posting() {
        let app = test_app();
        setup_accounts(&app).await;
        put_account(&app, CASH_ID, None, "Cash", "asset").await;
        put_account(&app, CHECKING_ID, Some(CASH_ID), "Checking", "asset").await;
        put_account(&app, SAVINGS_ID, Some(CASH_ID), "Savings", "asset").await;

        put_source(&app, -10_000, "Transfer to savings").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(json!({
                "changes": [
                    { "operation": "split", "target_id": TRANSFER_POSTING_ID },
                    {
                        "operation": "set",
                        "target_id": TRANSFER_POSTING_ID,
                        "field": "account_id",
                        "value": SAVINGS_ID
                    },
                    {
                        "operation": "set",
                        "target_id": TRANSFER_POSTING_ID,
                        "field": "amount",
                        "value": 10_000
                    }
                ]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let mut savings_source = source(2_500, "Interest");
        savings_source["external_account_id"] = json!("savings-123");
        savings_source["external_transaction_id"] = json!("interest-1");
        savings_source["account_id"] = json!(SAVINGS_ID);
        let (status, _) = request(
            &app,
            Method::PUT,
            &format!("/sources/{SECOND_SOURCE_ID}"),
            Some(savings_source),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, group) = request(
            &app,
            Method::GET,
            &format!("/accounts/{CASH_ID}/transactions"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let transactions = group["transactions"].as_array().unwrap();
        assert_eq!(transactions.len(), 3);

        let transfer = transactions
            .iter()
            .filter(|transaction| transaction["source_id"] == SOURCE_ID)
            .collect::<Vec<_>>();
        assert_eq!(transfer.len(), 2);
        assert!(
            transfer
                .iter()
                .any(|transaction| transaction["account_id"] == CHECKING_ID)
        );
        assert!(
            transfer
                .iter()
                .any(|transaction| transaction["account_id"] == SAVINGS_ID)
        );

        let interest = transactions
            .iter()
            .find(|transaction| transaction["source_id"] == SECOND_SOURCE_ID)
            .unwrap();
        assert_eq!(interest["account_id"], SAVINGS_ID);

        let (status, savings) = request(
            &app,
            Method::GET,
            &format!("/accounts/{SAVINGS_ID}/transactions"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(savings["transactions"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn all_transactions_register_includes_every_matching_posting() {
        let app = test_app();
        setup_accounts(&app).await;
        put_account(&app, CARD_ID, None, "Credit card", "liability").await;

        put_source(&app, -10_000, "Target").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(6_000, None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let mut card_source = source(-500, "Coffee");
        card_source["external_account_id"] = json!("card-123");
        card_source["external_transaction_id"] = json!("coffee-1");
        card_source["account_id"] = json!(CARD_ID);
        let (status, _) = request(
            &app,
            Method::PUT,
            &format!("/sources/{THIRD_SOURCE_ID}"),
            Some(card_source),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, all) = request(&app, Method::GET, "/transactions", None).await;
        assert_eq!(status, StatusCode::OK);
        let transactions = all["transactions"].as_array().unwrap();
        assert_eq!(transactions.len(), 3);
        assert!(
            transactions
                .iter()
                .any(|transaction| transaction["account_id"] == CHECKING_ID)
        );
        assert!(
            transactions
                .iter()
                .any(|transaction| transaction["account_id"] == CARD_ID)
        );
        assert!(
            transactions
                .iter()
                .any(|transaction| transaction["account_id"] == GROCERIES_ID)
        );
    }

    #[tokio::test]
    async fn deleting_local_account_preserves_transactions_with_null_postings() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(10_000, None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, deletion) = request(
            &app,
            Method::DELETE,
            &format!("/accounts/{GROCERIES_ID}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deletion["accounts"], 1);
        assert_eq!(deletion["transactions"], 0);
        assert_eq!(deletion["nulled_postings"], 1);

        let (status, source) =
            request(&app, Method::GET, &format!("/sources/{SOURCE_ID}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(source["postings"][1]["account_id"], Value::Null);

        let source = put_source(&app, -12_000, "Target after reimport").await;
        assert_eq!(source["postings"][1]["account_id"], Value::Null);

        let (status, unbalanced) =
            request(&app, Method::GET, "/transactions/unbalanced", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(unbalanced["transactions"].as_array().unwrap().len(), 2);
        assert!(
            unbalanced["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|transaction| transaction["account_id"] == Value::Null)
        );
        let (status, count) =
            request(&app, Method::GET, "/transactions/unbalanced/count", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(count["count"], 1);
    }

    #[tokio::test]
    async fn deleting_local_account_can_delete_affected_transactions() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;

        let (status, deletion) = request(
            &app,
            Method::DELETE,
            &format!("/accounts/{CHECKING_ID}?delete_transactions=true"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deletion["accounts"], 1);
        assert_eq!(deletion["transactions"], 1);

        let (status, _) = request(&app, Method::GET, &format!("/sources/{SOURCE_ID}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deleting_a_category_cannot_delete_transactions() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(10_000, None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = request(
            &app,
            Method::DELETE,
            &format!("/accounts/{GROCERIES_ID}?delete_transactions=true"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let (status, _) = request(&app, Method::GET, &format!("/sources/{SOURCE_ID}"), None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn removing_splits_returns_to_the_source_posting() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(6000, None)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(json!({
                "changes": [{
                    "operation": "remove_split",
                    "target_id": GROCERIES_POSTING_ID
                }]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["postings"].as_array().unwrap().len(), 1);
        assert_eq!(body["postings"][0]["id"], SOURCE_ID);
        assert_eq!(body["unallocated_amount"], 10_000);
    }

    #[tokio::test]
    async fn source_category_does_not_create_an_account() {
        let app = test_app();
        setup_accounts(&app).await;
        let body = put_source(&app, -10_000, "Target").await;
        assert_eq!(body["source"]["category"], "Shopping");

        let (status, accounts) = request(&app, Method::GET, "/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(accounts.as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn pending_is_inherited_and_deletion_cascades_postings() {
        let app = test_app();
        setup_accounts(&app).await;
        let mut input = source(-10_000, "Target");
        input["pending"] = json!(true);
        let (status, body) = request(
            &app,
            Method::PUT,
            &format!("/sources/{SOURCE_ID}"),
            Some(input),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["postings"][0]["pending"], true);

        let (status, _) =
            request(&app, Method::DELETE, &format!("/sources/{SOURCE_ID}"), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, postings) = request(
            &app,
            Method::GET,
            &format!("/accounts/{CHECKING_ID}/transactions"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(postings["transactions"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_change_batches_roll_back_completely() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;

        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(json!({
                "changes": [{
                    "operation": "split",
                    "target_id": GROCERIES_POSTING_ID
                }]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let (status, body) =
            request(&app, Method::GET, &format!("/sources/{SOURCE_ID}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["changes"].as_array().unwrap().is_empty());
        assert_eq!(body["postings"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn posted_transaction_can_replace_pending_source_identity() {
        use crate::model::{ImportedAccount, ImportedTransaction, ImportedTransactions};

        let mut database = Database::open(":memory:").unwrap();
        let account = database
            .sync_accounts(&[ImportedAccount {
                id: "bank".into(),
                parent_id: None,
                name: "Checking".into(),
                account_type: AccountType::Asset,
            }])
            .unwrap()
            .remove(0)
            .0;
        let pending = ImportedTransaction {
            id: "pending-1".into(),
            replaces_pending_id: None,
            date: "2026-08-04".into(),
            payee: Some("COFFEE SHOP PENDING".into()),
            amount: -500,
            category: None,
            pending: true,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let pending_result = database
            .sync_transactions(
                &account,
                ImportedTransactions {
                    transactions: vec![pending],
                    pending_ids: vec!["pending-1".into()],
                },
                None,
            )
            .unwrap();
        let source_id = pending_result.new_source_ids[0].clone();
        database
            .append_changes(
                &source_id,
                vec![Change::Set {
                    target_id: source_id.clone(),
                    field: "payee".into(),
                    value: json!("Coffee Shop"),
                }],
            )
            .unwrap();
        assert!(database.payee_examples().unwrap().is_empty());

        let posted = ImportedTransaction {
            id: "posted-1".into(),
            replaces_pending_id: Some("pending-1".into()),
            date: "2026-08-05".into(),
            payee: Some("COFFEE SHOP".into()),
            amount: -500,
            category: None,
            pending: false,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let posted_result = database
            .sync_transaction_batch(&[(&account, &posted)], None)
            .unwrap();
        let deleted_pending = database
            .cleanup_pending_transactions(&[(&account, &[])])
            .unwrap();

        assert_eq!(deleted_pending, 0);
        assert_eq!(database.payee_examples().unwrap().len(), 1);
        assert!(posted_result.new_source_ids.is_empty());
        let bundle = database.get_source(&source_id).unwrap();
        assert_eq!(bundle.source.external_transaction_id, "posted-1");
        assert!(!bundle.source.transaction.pending);
        assert_eq!(bundle.changes.len(), 2);
        assert!(
            bundle
                .changes
                .iter()
                .any(|change| matches!(change.change, Change::Log { .. }))
        );
        assert_eq!(
            bundle.projection.postings[0].posting.payee.as_deref(),
            Some("Coffee Shop")
        );
    }

    #[test]
    fn automatic_payee_sets_train_the_next_batch_and_can_be_bypassed() {
        use crate::model::{ImportedAccount, ImportedTransaction, ImportedTransactions};
        use monet_server::payee_resolution::PayeeResolver;

        let imported = |id: &str, payee: &str| ImportedTransaction {
            id: id.into(),
            replaces_pending_id: None,
            date: "2026-08-05".into(),
            payee: Some(payee.into()),
            amount: -500,
            category: None,
            pending: false,
            opening_balance: false,
            notes: None,
            raw: Value::Null,
        };
        let mut database = Database::open(":memory:").unwrap();
        let account = database
            .sync_accounts(&[ImportedAccount {
                id: "bank".into(),
                parent_id: None,
                name: "Checking".into(),
                account_type: AccountType::Asset,
            }])
            .unwrap()
            .remove(0)
            .0;

        let empty_resolver = PayeeResolver::new(&[]);
        let first_result = database
            .sync_transactions(
                &account,
                ImportedTransactions {
                    transactions: vec![imported("tx-1", "POS PURCHASE STARBUCKS #1000")],
                    pending_ids: vec![],
                },
                Some(&empty_resolver),
            )
            .unwrap();
        assert!(first_result.resolved_payees.is_empty());
        assert_eq!(first_result.unresolved_payees.len(), 1);
        assert_eq!(
            first_result.unresolved_payees[0].source_id,
            first_result.new_source_ids[0]
        );
        assert_eq!(
            first_result.unresolved_payees[0].imported_payee,
            "POS PURCHASE STARBUCKS #1000"
        );
        assert!(
            database
                .append_payee_resolutions(&[
                    (first_result.new_source_ids[0].clone(), "Starbucks".into()),
                    ("missing-source".into(), "Wrong".into()),
                ])
                .is_err()
        );
        assert!(
            database
                .get_source(&first_result.new_source_ids[0])
                .unwrap()
                .changes
                .is_empty()
        );
        database
            .append_changes(
                &first_result.new_source_ids[0],
                vec![Change::Set {
                    target_id: first_result.new_source_ids[0].clone(),
                    field: "payee".into(),
                    value: json!("Starbucks"),
                }],
            )
            .unwrap();

        let resolver = PayeeResolver::new(&database.payee_examples().unwrap());
        let second_result = database
            .sync_transactions(
                &account,
                ImportedTransactions {
                    transactions: vec![imported("tx-2", "POS PURCHASE STARBUCKS #2000")],
                    pending_ids: vec![],
                },
                Some(&resolver),
            )
            .unwrap();
        assert_eq!(second_result.resolved_payees.len(), 1);
        assert_eq!(second_result.resolved_payees[0].resolved_payee, "Starbucks");
        assert!(second_result.unresolved_payees.is_empty());
        let second = database
            .get_source(&second_result.new_source_ids[0])
            .unwrap();
        assert_eq!(
            second.source.transaction.payee.as_deref(),
            Some("POS PURCHASE STARBUCKS #2000")
        );
        assert_eq!(
            second.projection.postings[0].posting.payee.as_deref(),
            Some("Starbucks")
        );
        assert!(matches!(
            &second.changes[0].change,
            Change::Set { field, value, .. } if field == "payee" && value == "Starbucks"
        ));
        assert_eq!(database.payee_examples().unwrap().len(), 2);

        let third_result = database
            .sync_transactions(
                &account,
                ImportedTransactions {
                    transactions: vec![imported("tx-3", "POS PURCHASE STARBUCKS #3000")],
                    pending_ids: vec![],
                },
                None,
            )
            .unwrap();
        assert!(third_result.resolved_payees.is_empty());
        assert!(third_result.unresolved_payees.is_empty());
        let third = database
            .get_source(&third_result.new_source_ids[0])
            .unwrap();
        assert_eq!(
            third.projection.postings[0].posting.payee.as_deref(),
            Some("POS PURCHASE STARBUCKS #3000")
        );
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
