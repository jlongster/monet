use std::time::Instant;

use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};

use crate::{
    config::ConfigFile,
    db::{Database, DatabaseError, TransactionSumFilters},
    error::MonetError,
    model::{
        AccountCursor, AccountDeletion, AccountInput, AccountRecord, AccountTransactionsPage,
        AccountType, ChangesInput, ImportPluginConfig, ImportSummary, InferenceConfig,
        InferenceGenerateInput, InferenceGenerateOutput, InferenceModelInput, InferenceModels,
        ModelError, PluginStatus, PluginsConfigFile, SourceBundle, SourceInput,
    },
    operations,
    plugin::PluginError,
    state::{ImportLog, MonetState},
};

#[derive(Serialize)]
struct Health {
    healthy: bool,
}

#[derive(Serialize)]
struct TransactionCount {
    count: usize,
}

#[derive(Serialize)]
struct TransactionSum {
    sum: i64,
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

impl IntoResponse for MonetError {
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
pub(crate) fn create_router(database: Database, config: ConfigFile) -> Router {
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
        .route("/payees", get(list_payees))
        .route("/sum", get(sum_transactions))
        .route("/mcp", post(crate::mcp::handle))
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
        .with_state(MonetState::new(database, config))
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

async fn get_plugins(
    State(state): State<MonetState>,
) -> Result<Json<PluginsConfigFile>, MonetError> {
    let config = state.config.load()?;
    Ok(Json(PluginsConfigFile {
        path: state.config.path(),
        inference: config.inference,
        import: config.import,
    }))
}

fn require_inference_config(state: &MonetState) -> Result<InferenceConfig, MonetError> {
    state
        .config
        .load()?
        .inference
        .ok_or_else(|| PluginError::NotConfigured.into())
}

fn require_import_config(state: &MonetState) -> Result<ImportPluginConfig, MonetError> {
    state
        .config
        .load()?
        .import
        .ok_or_else(|| PluginError::NotConfigured.into())
}

async fn inference_status(
    State(state): State<MonetState>,
) -> Result<Json<PluginStatus>, MonetError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.status(config).await?))
}

async fn inference_models(
    State(state): State<MonetState>,
) -> Result<Json<InferenceModels>, MonetError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.models(config).await?))
}

async fn update_inference_model(
    State(state): State<MonetState>,
    Json(input): Json<InferenceModelInput>,
) -> Result<Json<InferenceConfig>, MonetError> {
    Ok(Json(state.config.set_inference_model(input.model)?))
}

async fn inference_generate(
    State(state): State<MonetState>,
    Json(input): Json<InferenceGenerateInput>,
) -> Result<Json<InferenceGenerateOutput>, MonetError> {
    let config = require_inference_config(&state)?;
    Ok(Json(state.inference.generate(config, input).await?))
}

async fn inference_restart(State(state): State<MonetState>) -> Json<Health> {
    state.inference.restart().await;
    Json(Health { healthy: true })
}

async fn import_status(State(state): State<MonetState>) -> Result<Json<PluginStatus>, MonetError> {
    let config = require_import_config(&state)?;
    Ok(Json(state.importer.status(config).await?))
}

async fn import_restart(State(state): State<MonetState>) -> Json<Health> {
    state.importer.restart().await;
    Json(Health { healthy: true })
}

async fn run_import(
    State(state): State<MonetState>,
    input: Option<Json<ImportRunInput>>,
) -> Result<Json<ImportSummary>, MonetError> {
    operations::run_import(&state, input.and_then(|input| input.end_date.clone()))
        .await
        .map(Json)
}

async fn import_logs(State(state): State<MonetState>) -> Json<ImportLog> {
    Json(
        state
            .import_log
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default(),
    )
}

async fn upsert_account(
    State(state): State<MonetState>,
    AxumPath(account_id): AxumPath<String>,
    Json(input): Json<AccountInput>,
) -> Result<Json<AccountRecord>, MonetError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.upsert_account(&account_id, input)?))
}

async fn list_accounts(
    State(state): State<MonetState>,
) -> Result<Json<Vec<AccountRecord>>, MonetError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_accounts()?))
}

async fn list_payees(State(state): State<MonetState>) -> Result<Json<Vec<String>>, MonetError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_payees()?))
}

async fn delete_account(
    State(state): State<MonetState>,
    AxumPath(account_id): AxumPath<String>,
    Query(query): Query<DeleteAccountQuery>,
) -> Result<Json<AccountDeletion>, MonetError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(
        database.delete_account(&account_id, query.delete_transactions)?,
    ))
}

async fn upsert_source(
    State(state): State<MonetState>,
    AxumPath(source_id): AxumPath<String>,
    Json(input): Json<SourceInput>,
) -> Result<Json<SourceBundle>, MonetError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.upsert_source(&source_id, input)?))
}

async fn get_source(
    State(state): State<MonetState>,
    AxumPath(source_id): AxumPath<String>,
) -> Result<Json<SourceBundle>, MonetError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.get_source(&source_id)?))
}

async fn delete_pending_source(
    State(state): State<MonetState>,
    AxumPath(source_id): AxumPath<String>,
) -> Result<StatusCode, MonetError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    database.delete_pending_source(&source_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn append_changes(
    State(state): State<MonetState>,
    AxumPath(source_id): AxumPath<String>,
    Json(input): Json<ChangesInput>,
) -> Result<Json<SourceBundle>, MonetError> {
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.append_changes(&source_id, input.changes)?))
}

async fn list_account_transactions(
    State(state): State<MonetState>,
    AxumPath(account_id): AxumPath<String>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, MonetError> {
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
    State(state): State<MonetState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, MonetError> {
    let (limit, before, account_types, search) = transaction_page(query)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_all_transactions(
        limit,
        before.as_ref(),
        account_types.as_deref(),
        search.as_deref(),
    )?))
}

async fn sum_transactions(
    State(state): State<MonetState>,
    Query(parameters): Query<Vec<(String, String)>>,
) -> Result<Json<TransactionSum>, MonetError> {
    let filters = transaction_sum_filters(parameters)?;
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(TransactionSum {
        sum: database.sum_transactions(&filters)?,
    }))
}

async fn list_unbalanced_transactions(
    State(state): State<MonetState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<AccountTransactionsPage>, MonetError> {
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
    State(state): State<MonetState>,
    Query(query): Query<AccountPageQuery>,
) -> Result<Json<TransactionCount>, MonetError> {
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
    MonetError,
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

fn transaction_sum_filters(
    parameters: Vec<(String, String)>,
) -> Result<TransactionSumFilters, MonetError> {
    let mut filters = TransactionSumFilters::default();
    for (name, value) in parameters {
        match name.as_str() {
            "account" => {
                if value.is_empty() {
                    return Err(invalid_filter("account", "a non-empty account ID"));
                }
                if !filters.account_ids.contains(&value) {
                    filters.account_ids.push(value);
                }
            }
            "amount[gt]" => {
                set_filter(&mut filters.amount_gt, parse_amount(&name, &value)?, &name)?
            }
            "amount[gte]" => {
                set_filter(&mut filters.amount_gte, parse_amount(&name, &value)?, &name)?
            }
            "amount[lt]" => {
                set_filter(&mut filters.amount_lt, parse_amount(&name, &value)?, &name)?
            }
            "amount[lte]" => {
                set_filter(&mut filters.amount_lte, parse_amount(&name, &value)?, &name)?
            }
            "date[gt]" => {
                validate_date(&name, &value)?;
                set_filter(&mut filters.date_gt, value, &name)?;
            }
            "date[gte]" => {
                validate_date(&name, &value)?;
                set_filter(&mut filters.date_gte, value, &name)?;
            }
            "date[lt]" => {
                validate_date(&name, &value)?;
                set_filter(&mut filters.date_lt, value, &name)?;
            }
            "date[lte]" => {
                validate_date(&name, &value)?;
                set_filter(&mut filters.date_lte, value, &name)?;
            }
            _ => return Err(invalid_filter(&name, "a supported sum filter")),
        }
    }
    Ok(filters)
}

fn set_filter<T>(slot: &mut Option<T>, value: T, field: &str) -> Result<(), MonetError> {
    if slot.is_some() {
        return Err(invalid_filter(field, "a single value"));
    }
    *slot = Some(value);
    Ok(())
}

fn parse_amount(field: &str, value: &str) -> Result<i64, MonetError> {
    value
        .parse()
        .map_err(|_| invalid_filter(field, "a signed integer number of cents"))
}

fn validate_date(field: &str, value: &str) -> Result<(), MonetError> {
    let [year, month, day] = value
        .split('-')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| invalid_filter(field, "a date in YYYY-MM-DD format"))?;
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return Err(invalid_filter(field, "a date in YYYY-MM-DD format"));
    }
    let year: u32 = year
        .parse()
        .map_err(|_| invalid_filter(field, "a valid date in YYYY-MM-DD format"))?;
    let month: usize = month
        .parse()
        .map_err(|_| invalid_filter(field, "a valid date in YYYY-MM-DD format"))?;
    let day: u32 = day
        .parse()
        .map_err(|_| invalid_filter(field, "a valid date in YYYY-MM-DD format"))?;
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap_year { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if month == 0 || month > days.len() || day == 0 || day > days[month - 1] {
        return Err(invalid_filter(field, "a valid date in YYYY-MM-DD format"));
    }
    Ok(())
}

fn invalid_filter(field: &str, expected: &'static str) -> MonetError {
    DatabaseError::Model(ModelError::InvalidValue {
        field: field.into(),
        expected,
    })
    .into()
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
    use crate::model::Change;

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
    async fn sum_filters_projected_postings_by_accounts_amount_and_date() {
        let app = test_app();
        setup_accounts(&app).await;
        put_source(&app, -10_000, "Target").await;
        let (status, _) = request(
            &app,
            Method::POST,
            &format!("/sources/{SOURCE_ID}/changes"),
            Some(split_changes(6_000, Some(4_000))),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        for (query, expected) in [
            ("", 0),
            (&format!("?account={GROCERIES_ID}"), 6_000),
            (
                &format!("?account={GROCERIES_ID}&account={HOUSEHOLD_ID}"),
                10_000,
            ),
            ("?amount%5Bgte%5D=4000&amount%5Blte%5D=6000", 10_000),
            ("?amount%5Bgt%5D=6000", 0),
            (
                &format!("?account={GROCERIES_ID}&date%5Bgte%5D=2026-07-20"),
                6_000,
            ),
            (
                &format!("?account={GROCERIES_ID}&date%5Bgt%5D=2026-07-20"),
                0,
            ),
        ] {
            let (status, body) = request(&app, Method::GET, &format!("/sum{query}"), None).await;
            assert_eq!(status, StatusCode::OK, "query: {query}");
            assert_eq!(body["sum"], expected, "query: {query}");
        }
    }

    #[tokio::test]
    async fn sum_rejects_invalid_filters() {
        let app = test_app();
        setup_accounts(&app).await;

        for query in [
            "amount%5Bgte%5D=one",
            "amount%5Bgte%5D=1&amount%5Bgte%5D=2",
            "date%5Blte%5D=2026-02-30",
            "payee=Target",
            "account=unknown",
        ] {
            let (status, _) = request(&app, Method::GET, &format!("/sum?{query}"), None).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "query: {query}");
        }
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
        let (status, payees) = request(&app, Method::GET, "/payees", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(payees, json!(["Coffee", "Target"]));
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
}
