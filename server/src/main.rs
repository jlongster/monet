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
        ImportTransactionsInput, InferenceConfig, InferenceGenerateInput, InferenceGenerateOutput,
        InferenceModelInput, InferenceModels, ModelError, PluginStatus, PluginsConfigFile,
        SourceBundle, SourceInput,
    },
    plugin::{ImportManager, InferenceManager, PluginError},
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
    account_types: Option<String>,
    search: Option<String>,
}

#[derive(Deserialize)]
struct DeleteAccountQuery {
    #[serde(default)]
    delete_transactions: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitialTransferReview {
    not_transfers: Vec<String>,
}

const OPENING_BALANCES_ACCOUNT_ID: &str = "b699843f-11bd-54e8-888b-452cb32be97b";

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

async fn run_import(State(state): State<AppState>) -> Result<Json<ImportSummary>, ApiError> {
    if let Ok(mut log) = state.import_log.lock() {
        log.running = true;
        log.lines.clear();
        log.lines.push("Starting transaction sync".into());
    }
    app_log::write("sync", "Starting transaction sync");
    let result = run_import_inner(&state).await;
    let completion = match &result {
        Ok(summary) => format!(
            "Sync complete: {} accounts, {} transactions, {} categorized, {} pending removed",
            summary.accounts, summary.transactions, summary.categorized, summary.deleted_pending
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

async fn run_import_inner(state: &AppState) -> Result<ImportSummary, ApiError> {
    log_import(state, "Loading transaction import plugin configuration");
    let config = require_import_config(&state)?;
    log_import(state, "Requesting account snapshot from import plugin");
    let imported_accounts = state.importer.accounts(config.clone()).await?;
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
    let new_account_ids = accounts
        .iter()
        .filter(|(_, initial)| *initial)
        .map(|(account, _)| account.id.clone())
        .collect::<Vec<_>>();
    log_import(
        state,
        format!(
            "Registered {} accounts ({} new)",
            accounts.len(),
            new_account_ids.len()
        ),
    );

    let mut summary = ImportSummary {
        accounts: accounts.len(),
        transactions: 0,
        deleted_pending: 0,
        categorized: 0,
    };
    let mut new_source_ids = Vec::new();
    for (account, initial) in accounts {
        let account_id = account
            .external_id
            .as_deref()
            .ok_or_else(|| DatabaseError::AccountNotFound(account.id.clone()))?;
        let start_date = if initial {
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
        let imported = state
            .importer
            .transactions(
                config.clone(),
                ImportTransactionsInput {
                    account_id: account_id.to_owned(),
                    initial,
                    start_date,
                },
            )
            .await?;
        let returned = imported.transactions.len();
        let (transactions, deleted_pending, account_new_source_ids) = {
            let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            database.sync_transactions(&account, imported)?
        };
        summary.transactions += transactions;
        summary.deleted_pending += deleted_pending;
        log_import(
            state,
            format!(
                "{}: received {returned}, inserted {}, removed {} pending",
                account.name,
                account_new_source_ids.len(),
                deleted_pending
            ),
        );
        new_source_ids.extend(account_new_source_ids);
    }
    log_import(
        state,
        format!(
            "Import complete; {} new sources need counterparts",
            new_source_ids.len()
        ),
    );
    summary.categorized = process_initial_import(&state, &new_source_ids, &new_account_ids).await?;
    Ok(summary)
}

async fn process_initial_import(
    state: &AppState,
    new_source_ids: &[String],
    new_account_ids: &[String],
) -> Result<usize, ApiError> {
    let mut handled = 0;
    if !new_source_ids.is_empty() {
        let opening = {
            let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            new_source_ids
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
                }
            }
        }
    }

    if new_account_ids.is_empty() {
        log_import(state, "No new accounts; skipping initial transfer review");
        return Ok(handled);
    }
    let inference = match state.config.load()?.inference {
        Some(config) => config,
        None => {
            log_import(
                state,
                "Inference is not configured; skipping initial transfer review",
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
            database.transfer_pairs_for_accounts(new_account_ids)?,
        )
    };
    if pairs.is_empty() {
        log_import(state, "Initial transfer review found no candidate pairs");
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
            "Initial transfer review: {} asset/liability accounts, {} candidate pairs",
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
            log_import(state, format!("Initial transfer review failed: {error}"));
            return Ok(handled);
        }
    };
    let review = match response
        .structured
        .and_then(|value| serde_json::from_value::<InitialTransferReview>(value).ok())
    {
        Some(review) => review,
        None => {
            log_import(
                state,
                "Initial transfer review returned invalid structured output",
            );
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
            used_sources.insert(canonical.clone());
            used_sources.insert(duplicate.clone());
            handled += 2;
            log_import(
                state,
                format!("Reconciled initial transfer {alias}: {canonical} / {duplicate}"),
            );
        }
    }
    log_import(
        state,
        format!(
            "Initial transfer review complete: {} rejected, {} sources reconciled",
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
    let before = match (query.before_date, query.before_source_id) {
        (Some(date), Some(source_id)) => Some(AccountCursor { date, source_id }),
        (None, None) => None,
        _ => {
            return Err(DatabaseError::Model(ModelError::InvalidValue {
                field: "before".into(),
                expected: "both before_date and before_source_id",
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
    async fn parent_account_register_includes_descendants_without_duplicate_sources() {
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
        assert_eq!(transactions.len(), 2);

        let transfer = transactions
            .iter()
            .find(|transaction| transaction["source_id"] == SOURCE_ID)
            .unwrap();
        assert_eq!(transfer["account_id"], CHECKING_ID);
        assert_eq!(transfer["other_postings"].as_array().unwrap().len(), 1);
        assert_eq!(transfer["other_postings"][0]["account_id"], SAVINGS_ID);

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
    async fn all_transactions_register_includes_every_account_once_per_source() {
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
        assert_eq!(transactions.len(), 2);
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
        assert_eq!(unbalanced["transactions"].as_array().unwrap().len(), 1);
        assert_eq!(unbalanced["transactions"][0]["account_id"], Value::Null);
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
}
