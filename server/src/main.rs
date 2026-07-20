mod db;
mod model;
mod perf;

use std::{
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

use crate::{
    db::{Database, DatabaseError},
    model::{
        AccountCursor, AccountInput, AccountRecord, AccountTransactionsPage, ChangesInput,
        ImportConfig, ImportConfigInput, ImportSummary, ImportedAccounts, ImportedTransactions,
        ModelError, SourceBundle, SourceInput,
    },
};

#[derive(Clone)]
struct AppState {
    database: Arc<Mutex<Database>>,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct Health {
    healthy: bool,
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
}

struct ApiError(DatabaseError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            DatabaseError::SourceNotFound(_) | DatabaseError::AccountNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            DatabaseError::ImportNotConfigured => StatusCode::NOT_FOUND,
            DatabaseError::Model(_)
            | DatabaseError::Json(_)
            | DatabaseError::InvalidImportUrl(_) => StatusCode::UNPROCESSABLE_ENTITY,
            DatabaseError::ImportRequest(_) => StatusCode::BAD_GATEWAY,
            DatabaseError::Sql(_) | DatabaseError::Poisoned => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

impl From<DatabaseError> for ApiError {
    fn from(error: DatabaseError) -> Self {
        Self(error)
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
    let app = create_router(Database::open(&database_path)?);

    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("monet server listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn create_router(database: Database) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/import", put(configure_import).get(get_import))
        .route("/import/run", post(run_import))
        .route("/accounts", get(list_accounts))
        .route("/accounts/{account_id}", put(upsert_account))
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
            http: reqwest::Client::new(),
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

async fn configure_import(
    State(state): State<AppState>,
    Json(input): Json<ImportConfigInput>,
) -> Result<Json<ImportConfig>, ApiError> {
    let mut url = reqwest::Url::parse(&input.url)
        .map_err(|error| DatabaseError::InvalidImportUrl(error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(DatabaseError::InvalidImportUrl("URL must use HTTP or HTTPS".into()).into());
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.configure_import(url.to_string())?))
}

async fn get_import(State(state): State<AppState>) -> Result<Json<ImportConfig>, ApiError> {
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.get_import()?))
}

async fn run_import(State(state): State<AppState>) -> Result<Json<ImportSummary>, ApiError> {
    let config = {
        let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        database.get_import()?
    };
    let base = reqwest::Url::parse(&config.url)
        .map_err(|error| DatabaseError::InvalidImportUrl(error.to_string()))?;
    let accounts_url = base
        .join("accounts")
        .map_err(|error| DatabaseError::InvalidImportUrl(error.to_string()))?;
    let imported_accounts: ImportedAccounts = fetch_json(&state.http, accounts_url).await?;
    let accounts = {
        let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
        database.sync_accounts(&config, &imported_accounts.accounts)?
    };

    let mut summary = ImportSummary {
        accounts: accounts.len(),
        transactions: 0,
        deleted_pending: 0,
    };
    for account in accounts {
        let external_id = account
            .external_id
            .as_deref()
            .ok_or_else(|| DatabaseError::AccountNotFound(account.id.clone()))?;
        let mut url = base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| DatabaseError::InvalidImportUrl(config.url.clone()))?;
            segments.pop_if_empty();
            segments.push("accounts");
            segments.push(external_id);
            segments.push("transactions");
        }
        let start_date = {
            let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            database.account_sync_start_date(&account.id)?
        };
        url.query_pairs_mut().append_pair("start_date", &start_date);
        let imported: ImportedTransactions = fetch_json(&state.http, url).await?;
        let (transactions, deleted_pending) = {
            let mut database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
            database.sync_transactions(&config, &account, imported)?
        };
        summary.transactions += transactions;
        summary.deleted_pending += deleted_pending;
    }
    Ok(Json(summary))
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: reqwest::Url,
) -> Result<T, ApiError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| DatabaseError::ImportRequest(error.to_string()))?
        .error_for_status()
        .map_err(|error| DatabaseError::ImportRequest(error.to_string()))?;
    response
        .json()
        .await
        .map_err(|error| DatabaseError::ImportRequest(error.to_string()).into())
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
    let database = state.database.lock().map_err(|_| DatabaseError::Poisoned)?;
    Ok(Json(database.list_account(
        &account_id,
        query.limit.unwrap_or(50).clamp(1, 100),
        before.as_ref(),
    )?))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        body::Body,
        extract::Query,
        http::{Method, Request},
        routing::get,
    };
    use http_body_util::BodyExt;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    const SOURCE_ID: &str = "019bf123-0000-7000-8000-000000000001";
    const CHECKING_ID: &str = "019bf123-0000-7000-8000-000000000010";
    const EXPENSES_ID: &str = "019bf123-0000-7000-8000-000000000011";
    const GROCERIES_ID: &str = "019bf123-0000-7000-8000-000000000012";
    const HOUSEHOLD_ID: &str = "019bf123-0000-7000-8000-000000000013";
    const PRODUCE_ID: &str = "019bf123-0000-7000-8000-000000000014";
    const GROCERIES_POSTING_ID: &str = "019bf123-0000-7000-8000-000000000002";
    const HOUSEHOLD_POSTING_ID: &str = "019bf123-0000-7000-8000-000000000003";

    #[derive(Clone)]
    struct FakeImportState {
        start_dates: Arc<Mutex<Vec<Option<String>>>>,
    }

    #[derive(Deserialize)]
    struct ImportQuery {
        start_date: Option<String>,
    }

    fn test_app() -> Router {
        create_router(Database::open(":memory:").unwrap())
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

    #[tokio::test]
    async fn importer_syncs_accounts_cursors_pending_and_postings() {
        async fn accounts() -> Json<Value> {
            Json(json!({
                "accounts": [
                    { "id": "bank", "parent_id": null, "name": "Checking", "type": "asset" },
                    { "id": "expenses", "parent_id": null, "name": "Expenses", "type": "expense" },
                    { "id": "groceries", "parent_id": "expenses", "name": "Groceries", "type": "expense" }
                ]
            }))
        }

        async fn transactions(
            State(state): State<FakeImportState>,
            AxumPath(account_id): AxumPath<String>,
            Query(query): Query<ImportQuery>,
        ) -> Json<Value> {
            if account_id != "bank" {
                return Json(json!({
                    "transactions": [],
                    "pending_ids": [],
                    "latest": null
                }));
            }
            let mut requests = state.start_dates.lock().unwrap();
            requests.push(query.start_date.clone());
            if requests.len() == 1 {
                return Json(json!({
                    "transactions": [
                        {
                            "id": "tx-1",
                            "date": "2026-07-19",
                            "payee": "Target",
                            "amount": -10000,
                            "category": "Shopping",
                            "pending": false,
                            "notes": null,
                            "raw": {}
                        },
                        {
                            "id": "tx-pending",
                            "date": "2026-07-20",
                            "payee": "Coffee",
                            "amount": -500,
                            "category": null,
                            "pending": true,
                            "notes": null,
                            "raw": {}
                        }
                    ],
                    "pending_ids": ["tx-pending"],
                    "latest": "tx-1"
                }));
            }
            Json(json!({
                "transactions": [{
                    "id": "tx-2",
                    "date": "2026-07-21",
                    "payee": "Payroll",
                    "amount": 300000,
                    "category": "Salary",
                    "pending": false,
                    "notes": null,
                    "raw": {}
                }],
                "pending_ids": [],
                "latest": "tx-2"
            }))
        }

        let fake_state = FakeImportState {
            start_dates: Arc::new(Mutex::new(Vec::new())),
        };
        let fake_app = Router::new()
            .route("/accounts", get(accounts))
            .route("/accounts/{account_id}/transactions", get(transactions))
            .with_state(fake_state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, fake_app).await.unwrap() });

        let app = test_app();
        let (status, _) = request(
            &app,
            Method::PUT,
            "/import",
            Some(json!({ "url": format!("http://{address}") })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, first) = request(&app, Method::POST, "/import/run", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["accounts"], 3);
        assert_eq!(first["transactions"], 2);
        assert_eq!(first["deleted_pending"], 0);

        let (status, accounts) = request(&app, Method::GET, "/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        let checking = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["external_id"] == "bank")
            .unwrap();
        let checking_id = checking["id"].as_str().unwrap();
        assert_eq!(checking["latest_external_transaction_id"], "tx-1");

        let (status, second) = request(&app, Method::POST, "/import/run", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(second["transactions"], 1);
        assert_eq!(second["deleted_pending"], 1);

        let (status, entries) = request(
            &app,
            Method::GET,
            &format!("/accounts/{checking_id}/transactions?limit=1"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(entries["transactions"].as_array().unwrap().len(), 1);
        let next = &entries["next"];
        let (status, next_entries) = request(
            &app,
            Method::GET,
            &format!(
                "/accounts/{checking_id}/transactions?limit=1&before_date={}&before_source_id={}",
                next["date"].as_str().unwrap(),
                next["source_id"].as_str().unwrap()
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(next_entries["transactions"].as_array().unwrap().len(), 1);
        assert_eq!(next_entries["next"], Value::Null);
        assert_eq!(
            fake_state.start_dates.lock().unwrap().as_slice(),
            &[Some("2026-01-01".into()), Some("2026-07-12".into())]
        );
        server.abort();
    }
}
