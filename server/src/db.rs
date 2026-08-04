use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::model::{
    AccountCursor, AccountDeletion, AccountInput, AccountRecord, AccountTransaction,
    AccountTransactionsPage, AccountType, Change, ImportConfig, ImportedAccount,
    ImportedTransaction, ImportedTransactions, ModelError, OtherPosting, Posting, PostingData,
    Projection, SourceBundle, SourceInput, SourceRecord, SourceTransactionData, StoredChange,
    reduce,
};

pub struct Database {
    connection: Connection,
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid data: {0}")]
    Model(#[from] ModelError),
    #[error("source '{0}' was not found")]
    SourceNotFound(String),
    #[error("account '{0}' was not found")]
    AccountNotFound(String),
    #[error("synced account '{0}' cannot be deleted locally")]
    SyncedAccountDeletion(String),
    #[error("synced account '{0}' hierarchy and type are managed by its importer")]
    SyncedAccountStructure(String),
    #[error("deleting an income or expense account must preserve its transactions")]
    CategoryTransactionDeletion,
    #[error("importer is not configured")]
    ImportNotConfigured,
    #[error("invalid importer URL: {0}")]
    InvalidImportUrl(String),
    #[error("import request failed: {0}")]
    ImportRequest(String),
    #[error("database lock was poisoned")]
    Poisoned,
}

type EncodedChange<'a> = (&'static str, &'a str, Option<&'a str>, Option<String>);

impl Database {
    pub fn open(path: &str) -> Result<Self, DatabaseError> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        let account_columns = {
            let mut statement = connection.prepare("PRAGMA table_info(transactions_source)")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, bool>(3)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let account_id_required = account_columns
            .iter()
            .any(|(name, required)| name == "account_id" && *required);
        if account_id_required {
            connection.execute_batch(include_str!(
                "../migrations/002_nullable_account_references.sql"
            ))?;
        }
        let has_imported_name = {
            let mut statement = connection.prepare("PRAGMA table_info(accounts)")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|name| name == "imported_name")
        };
        if !has_imported_name {
            connection
                .execute_batch(include_str!("../migrations/003_imported_account_names.sql"))?;
        }
        Ok(Self { connection })
    }

    pub fn upsert_account(
        &mut self,
        id: &str,
        input: AccountInput,
    ) -> Result<AccountRecord, DatabaseError> {
        crate::model::validate_account(id, &input)?;
        let tx = self.connection.transaction()?;
        let synced_account = tx
            .query_row(
                "SELECT external_id, parent_id, type FROM accounts WHERE id = ?",
                [id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((Some(_), parent_id, account_type)) = synced_account {
            if parent_id != input.parent_id
                || AccountType::parse(&account_type)? != input.account_type
            {
                return Err(DatabaseError::SyncedAccountStructure(id.to_owned()));
            }
        }
        validate_account_hierarchy(&tx, id, input.parent_id.as_deref(), input.account_type)?;
        tx.execute(
            "INSERT INTO accounts (id, parent_id, name, type)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                parent_id = excluded.parent_id,
                name = excluded.name,
                type = excluded.type",
            params![id, input.parent_id, input.name, input.account_type.as_str()],
        )?;
        tx.commit()?;
        self.get_account(id)
    }

    pub fn delete_account(
        &mut self,
        id: &str,
        delete_transactions: bool,
    ) -> Result<AccountDeletion, DatabaseError> {
        ensure_account(&self.connection, id)?;
        let account_type =
            self.connection
                .query_row("SELECT type FROM accounts WHERE id = ?", [id], |row| {
                    row.get::<_, String>(0)
                })?;
        if delete_transactions
            && matches!(
                AccountType::parse(&account_type)?,
                AccountType::Income | AccountType::Expense
            )
        {
            return Err(DatabaseError::CategoryTransactionDeletion);
        }
        let tx = self.connection.transaction()?;
        let account_ids = descendant_account_ids(&tx, id)?;
        let synced = account_ids.iter().find_map(|account_id| {
            tx.query_row(
                "SELECT external_id FROM accounts WHERE id = ?",
                [account_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .map(|_| account_id.clone())
        });
        if let Some(account_id) = synced {
            return Err(DatabaseError::SyncedAccountDeletion(account_id));
        }
        let deletion = delete_accounts(&tx, &account_ids, delete_transactions)?;
        tx.commit()?;
        Ok(deletion)
    }

    pub fn configure_import(&mut self, url: String) -> Result<ImportConfig, DatabaseError> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM import_config WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        self.connection.execute(
            "INSERT INTO import_config (singleton, id, url) VALUES (1, ?, ?)
             ON CONFLICT(singleton) DO UPDATE SET url = excluded.url",
            params![id, url],
        )?;
        self.get_import()
    }

    pub fn get_import(&self) -> Result<ImportConfig, DatabaseError> {
        self.connection
            .query_row(
                "SELECT id, url FROM import_config WHERE singleton = 1",
                [],
                |row| {
                    Ok(ImportConfig {
                        id: row.get(0)?,
                        url: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(DatabaseError::ImportNotConfigured)
    }

    pub fn sync_accounts(
        &mut self,
        config: &ImportConfig,
        accounts: &[ImportedAccount],
    ) -> Result<Vec<AccountRecord>, DatabaseError> {
        let namespace = parse_namespace(&config.id)?;
        let mut external_ids = HashSet::new();
        for account in accounts {
            validate_external_id(&account.id)?;
            if !external_ids.insert(account.id.as_str()) {
                return Err(ModelError::InvalidExternalId.into());
            }
            if account.name.trim().is_empty() {
                return Err(ModelError::InvalidAccountName.into());
            }
        }

        let tx = self.connection.transaction()?;
        let missing_accounts = {
            let mut statement =
                tx.prepare("SELECT id, external_id FROM accounts WHERE external_id IS NOT NULL")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|(id, external_id)| {
                    *id == imported_uuid(&namespace, "account", external_id)
                        && !external_ids.contains(external_id.as_str())
                })
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };
        for account_id in missing_accounts {
            let exists = tx
                .query_row("SELECT 1 FROM accounts WHERE id = ?", [&account_id], |_| {
                    Ok(())
                })
                .optional()?
                .is_some();
            if !exists {
                continue;
            }
            let account_ids = descendant_account_ids(&tx, &account_id)?;
            let placeholders = std::iter::repeat_n("?", account_ids.len())
                .collect::<Vec<_>>()
                .join(",");
            let external_sql = format!(
                "SELECT external_id FROM accounts
                 WHERE id IN ({placeholders}) AND external_id IS NOT NULL"
            );
            let account_values: Vec<SqlValue> =
                account_ids.iter().cloned().map(SqlValue::Text).collect();
            let removed_external_ids = {
                let mut statement = tx.prepare(&external_sql)?;
                let rows = statement.query_map(params_from_iter(account_values.iter()), |row| {
                    row.get::<_, String>(0)
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for external_id in removed_external_ids {
                tx.execute(
                    "DELETE FROM transactions_source WHERE source = ? AND external_account_id = ?",
                    params![config.id, external_id],
                )?;
            }
            delete_accounts(&tx, &account_ids, false)?;
        }
        for account in accounts {
            let id = imported_uuid(&namespace, "account", &account.id);
            tx.execute(
                "INSERT INTO accounts (id, parent_id, external_id, name, imported_name, type)
                  VALUES (?, NULL, ?, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                     parent_id = NULL,
                     external_id = excluded.external_id,
                     imported_name = excluded.imported_name,
                     type = excluded.type",
                params![
                    id,
                    account.id,
                    account.name,
                    account.name,
                    account.account_type.as_str()
                ],
            )?;
        }
        for account in accounts {
            let id = imported_uuid(&namespace, "account", &account.id);
            let parent_id = account
                .parent_id
                .as_deref()
                .map(|parent| imported_uuid(&namespace, "account", parent));
            if let Some(parent_id) = &parent_id {
                ensure_account(&tx, parent_id)?;
            }
            validate_account_hierarchy(&tx, &id, parent_id.as_deref(), account.account_type)?;
            tx.execute(
                "UPDATE accounts SET parent_id = ? WHERE id = ?",
                params![parent_id, id],
            )?;
        }
        tx.commit()?;

        accounts
            .iter()
            .map(|account| self.get_account(&imported_uuid(&namespace, "account", &account.id)))
            .collect()
    }

    pub fn sync_transactions(
        &mut self,
        config: &ImportConfig,
        account: &AccountRecord,
        imported: ImportedTransactions,
    ) -> Result<(usize, usize), DatabaseError> {
        let external_account_id = account
            .external_id
            .as_deref()
            .ok_or_else(|| DatabaseError::AccountNotFound(account.id.clone()))?;
        let namespace = parse_namespace(&config.id)?;
        let mut transaction_ids = HashSet::new();
        let pending_ids: HashSet<&str> = imported.pending_ids.iter().map(String::as_str).collect();
        for transaction in &imported.transactions {
            validate_external_id(&transaction.id)?;
            if transaction.date.is_empty() || !transaction_ids.insert(transaction.id.as_str()) {
                return Err(ModelError::InvalidExternalId.into());
            }
            if transaction.pending && !pending_ids.contains(transaction.id.as_str()) {
                return Err(ModelError::InvalidExternalId.into());
            }
        }

        let tx = self.connection.transaction()?;
        for transaction in &imported.transactions {
            let source_id = imported_uuid(
                &namespace,
                "transaction",
                &format!("{external_account_id}:{}", transaction.id),
            );
            let input = imported_source(config, account, transaction);
            upsert_source(&tx, &source_id, &input)?;
            rebuild_projection(&tx, &source_id)?;
        }

        let pending_sources = {
            let mut statement = tx.prepare(
                "SELECT id, external_transaction_id
                 FROM transactions_source
                 WHERE account_id = ? AND source = ? AND pending = 1",
            )?;
            let rows = statement.query_map(params![account.id, config.id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut deleted_pending = 0;
        for (source_id, external_id) in pending_sources {
            if !pending_ids.contains(external_id.as_str()) {
                tx.execute("DELETE FROM transactions_source WHERE id = ?", [&source_id])?;
                deleted_pending += 1;
            }
        }
        tx.execute(
            "UPDATE accounts SET latest_external_transaction_id = ? WHERE id = ?",
            params![imported.latest, account.id],
        )?;
        tx.commit()?;
        Ok((imported.transactions.len(), deleted_pending))
    }

    pub fn account_sync_start_date(&self, account_id: &str) -> Result<String, DatabaseError> {
        ensure_account(&self.connection, account_id)?;
        self.connection
            .query_row(
                "WITH dates AS (
                    SELECT
                        MAX(CASE WHEN pending = 0 AND opening_balance = 0 THEN date END) AS latest_posted,
                        MIN(CASE WHEN pending = 1 THEN date END) AS oldest_pending
                    FROM transactions_source
                    WHERE account_id = ?
                 )
                 SELECT CASE
                    WHEN latest_posted IS NULL THEN strftime('%Y-01-01', 'now')
                    ELSE date(MIN(latest_posted, COALESCE(oldest_pending, latest_posted)), '-7 days')
                 END
                 FROM dates",
                [account_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn account_needs_initial_sync(&self, account_id: &str) -> Result<bool, DatabaseError> {
        ensure_account(&self.connection, account_id)?;
        self.connection
            .query_row(
                "SELECT NOT EXISTS(
                    SELECT 1 FROM transactions_source WHERE account_id = ?
                 )",
                [account_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountRecord>, DatabaseError> {
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE descendants(root_id, id) AS (
                SELECT id, id FROM accounts
                UNION ALL
                SELECT descendants.root_id, accounts.id
                FROM descendants
                JOIN accounts ON accounts.parent_id = descendants.id
             ), balances AS (
                SELECT descendants.root_id, COALESCE(SUM(postings.amount), 0) AS balance
                FROM descendants
                LEFT JOIN postings ON postings.account_id = descendants.id
                GROUP BY descendants.root_id
             )
             SELECT
                accounts.id,
                accounts.parent_id,
                accounts.external_id,
                accounts.name,
                accounts.imported_name,
                accounts.type,
                accounts.latest_external_transaction_id,
                balances.balance,
                EXISTS(SELECT 1 FROM accounts AS child WHERE child.parent_id = accounts.id)
             FROM accounts
             JOIN balances ON balances.root_id = accounts.id
             ORDER BY accounts.name, accounts.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, bool>(8)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                parent_id,
                external_id,
                name,
                imported_name,
                account_type,
                latest,
                balance,
                has_children,
            ) = row?;
            Ok(AccountRecord {
                id,
                parent_id,
                external_id,
                name,
                imported_name,
                account_type: AccountType::parse(&account_type)?,
                latest_external_transaction_id: latest,
                balance,
                has_children,
            })
        })
        .collect()
    }

    pub fn upsert_source(
        &mut self,
        id: &str,
        input: SourceInput,
    ) -> Result<SourceBundle, DatabaseError> {
        crate::model::validate_source(id, &input)?;
        let tx = self.connection.transaction()?;
        upsert_source(&tx, id, &input)?;
        rebuild_projection(&tx, id)?;
        tx.commit()?;
        self.get_source(id)
    }

    pub fn append_changes(
        &mut self,
        source_id: &str,
        changes: Vec<Change>,
    ) -> Result<SourceBundle, DatabaseError> {
        let tx = self.connection.transaction()?;
        ensure_source(&tx, source_id)?;
        for change in changes {
            let (operation, target_id, field, value_json) = encode_change(&change)?;
            tx.execute(
                "INSERT INTO transactions_changes (
                    source_id, operation, target_id, field, value_json
                 ) VALUES (?, ?, ?, ?, ?)",
                params![source_id, operation, target_id, field, value_json],
            )?;
        }
        rebuild_projection(&tx, source_id)?;
        tx.commit()?;
        self.get_source(source_id)
    }

    pub fn delete_pending_source(&mut self, id: &str) -> Result<(), DatabaseError> {
        let pending = self
            .connection
            .query_row(
                "SELECT pending FROM transactions_source WHERE id = ?",
                [id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .ok_or_else(|| DatabaseError::SourceNotFound(id.to_owned()))?;
        if !pending {
            return Err(ModelError::SourceNotPending(id.to_owned()).into());
        }
        self.connection
            .execute("DELETE FROM transactions_source WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn get_source(&self, id: &str) -> Result<SourceBundle, DatabaseError> {
        let source = load_source(&self.connection, id)?;
        let changes = load_changes(&self.connection, id)?;
        let postings = load_postings(&self.connection, id)?;
        let expected = reduce(id, &source.transaction, &changes)?;

        Ok(SourceBundle {
            source,
            changes,
            projection: Projection {
                postings,
                unallocated_amount: expected.unallocated_amount,
            },
        })
    }

    pub fn list_account(
        &self,
        account_id: &str,
        limit: usize,
        before: Option<&AccountCursor>,
        search: Option<&str>,
    ) -> Result<AccountTransactionsPage, DatabaseError> {
        ensure_account(&self.connection, account_id)?;
        self.list_transactions(Some(account_id), limit, before, false, None, search)
    }

    pub fn list_all_transactions(
        &self,
        limit: usize,
        before: Option<&AccountCursor>,
        account_types: Option<&[AccountType]>,
        search: Option<&str>,
    ) -> Result<AccountTransactionsPage, DatabaseError> {
        self.list_transactions(None, limit, before, false, account_types, search)
    }

    pub fn list_unbalanced_transactions(
        &self,
        limit: usize,
        before: Option<&AccountCursor>,
        account_types: Option<&[AccountType]>,
        search: Option<&str>,
    ) -> Result<AccountTransactionsPage, DatabaseError> {
        self.list_transactions(None, limit, before, true, account_types, search)
    }

    pub fn count_unbalanced_transactions(
        &self,
        account_types: Option<&[AccountType]>,
        search: Option<&str>,
    ) -> Result<usize, DatabaseError> {
        let account_types = account_types.map(|types| {
            types
                .iter()
                .map(|account_type| account_type.as_str())
                .collect::<Vec<_>>()
                .join(",")
        });
        let search = search
            .map(str::trim)
            .filter(|search| !search.is_empty())
            .map(|search| format!("%{}%", search.to_lowercase()));
        self.connection
            .query_row(
                "SELECT COUNT(*)
                  FROM (
                     SELECT source_id
                     FROM postings
                     GROUP BY source_id
                     HAVING SUM(amount) != 0
                         OR SUM(CASE WHEN account_id IS NULL THEN 1 ELSE 0 END) > 0
                  ) AS unbalanced
                  WHERE (
                    ?1 IS NULL OR EXISTS (
                      SELECT 1
                      FROM postings AS typed_posting
                      JOIN accounts AS typed_account ON typed_account.id = typed_posting.account_id
                      WHERE typed_posting.source_id = unbalanced.source_id
                        AND instr(',' || ?1 || ',', ',' || typed_account.type || ',') > 0
                    )
                  )
                  AND (
                    ?2 IS NULL OR EXISTS (
                      SELECT 1
                      FROM postings AS search_posting
                      JOIN transactions_source AS search_source ON search_source.id = search_posting.source_id
                      WHERE search_posting.source_id = unbalanced.source_id
                        AND (
                          lower(COALESCE(search_posting.payee, '')) LIKE ?2
                          OR lower(COALESCE(search_source.payee, '')) LIKE ?2
                          OR lower(COALESCE(search_posting.notes, '')) LIKE ?2
                          OR lower(COALESCE(search_source.notes, '')) LIKE ?2
                          OR printf('%.2f', search_posting.amount / 100.0) LIKE ?2
                          OR printf('%.2f', ABS(search_posting.amount) / 100.0) LIKE ?2
                        )
                    )
                  )",
                params![account_types, search],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn list_transactions(
        &self,
        account_id: Option<&str>,
        limit: usize,
        before: Option<&AccountCursor>,
        unbalanced: bool,
        account_types: Option<&[AccountType]>,
        search: Option<&str>,
    ) -> Result<AccountTransactionsPage, DatabaseError> {
        let total_started = Instant::now();
        let base_started = Instant::now();
        let mut statement = self.connection.prepare(
            "WITH RECURSIVE descendants(id) AS (
                SELECT accounts.id
                FROM accounts
                WHERE ?1 IS NULL OR accounts.id = ?1
                UNION
                SELECT accounts.id
                FROM descendants
                JOIN accounts ON accounts.parent_id = descendants.id
             ), ranked_rows AS (
                SELECT
                    postings.source_id,
                    postings.id,
                    postings.account_id,
                    postings.date,
                    postings.payee,
                    postings.amount,
                    postings.pending,
                    postings.notes,
                    ROW_NUMBER() OVER (
                        PARTITION BY postings.source_id
                        ORDER BY
                            CASE WHEN ?5 = 1 AND postings.account_id IS NULL THEN 0 ELSE 1 END,
                            CASE WHEN postings.id = postings.source_id THEN 0 ELSE 1 END,
                            postings.account_id
                    ) AS source_rank
                FROM postings
                LEFT JOIN descendants ON descendants.id = postings.account_id
                LEFT JOIN accounts AS posting_account ON posting_account.id = postings.account_id
                WHERE (?5 = 1 OR descendants.id IS NOT NULL)
                  AND (
                    ?6 IS NULL
                    OR instr(',' || ?6 || ',', ',' || posting_account.type || ',') > 0
                  )
                  AND (
                    ?5 = 0
                    OR postings.source_id IN (
                        SELECT source_id
                        FROM postings
                        GROUP BY source_id
                        HAVING SUM(amount) != 0
                            OR SUM(CASE WHEN account_id IS NULL THEN 1 ELSE 0 END) > 0
                    )
                  )
                  AND (
                    ?2 IS NULL
                    OR postings.date < ?2
                    OR (postings.date = ?2 AND postings.source_id < ?3)
                  )
                  AND (
                    ?7 IS NULL OR EXISTS (
                      SELECT 1
                      FROM postings AS search_posting
                      JOIN transactions_source AS search_source ON search_source.id = search_posting.source_id
                      WHERE search_posting.source_id = postings.source_id
                        AND (
                          lower(COALESCE(search_posting.payee, '')) LIKE ?7
                          OR lower(COALESCE(search_source.payee, '')) LIKE ?7
                          OR lower(COALESCE(search_posting.notes, '')) LIKE ?7
                          OR lower(COALESCE(search_source.notes, '')) LIKE ?7
                          OR printf('%.2f', search_posting.amount / 100.0) LIKE ?7
                          OR printf('%.2f', ABS(search_posting.amount) / 100.0) LIKE ?7
                        )
                    )
                  )
             ), account_rows AS (
                SELECT source_id, id, account_id, date, payee, amount, pending, notes
                FROM ranked_rows
                WHERE source_rank = 1
                ORDER BY date DESC, source_id DESC
                LIMIT ?4
             )
             SELECT
                account_rows.source_id,
                account_rows.id,
                account_rows.account_id,
                account_rows.date,
                account_rows.payee,
                account_rows.amount,
                account_rows.pending,
                account_rows.notes,
                (SELECT COUNT(*) FROM postings AS all_postings
                 WHERE all_postings.source_id = account_rows.source_id),
                -(SELECT SUM(all_postings.amount) FROM postings AS all_postings
                  WHERE all_postings.source_id = account_rows.source_id)
             FROM account_rows
             ORDER BY account_rows.date DESC, account_rows.source_id DESC",
        )?;
        let before_date = before.map(|cursor| cursor.date.as_str());
        let before_source_id = before.map(|cursor| cursor.source_id.as_str());
        let account_types = account_types.map(|types| {
            types
                .iter()
                .map(|account_type| account_type.as_str())
                .collect::<Vec<_>>()
                .join(",")
        });
        let search = search
            .map(str::trim)
            .filter(|search| !search.is_empty())
            .map(|search| format!("%{}%", search.to_lowercase()));
        let rows = statement.query_map(
            params![
                account_id,
                before_date,
                before_source_id,
                limit + 1,
                unbalanced,
                account_types,
                search,
            ],
            |row| {
                Ok(AccountTransaction {
                    source_id: row.get(0)?,
                    posting_id: row.get(1)?,
                    account_id: row.get(2)?,
                    date: row.get(3)?,
                    payee: row.get(4)?,
                    amount: row.get(5)?,
                    pending: row.get(6)?,
                    notes: row.get(7)?,
                    posting_count: row.get(8)?,
                    unallocated_amount: row.get(9)?,
                    other_postings: Vec::new(),
                })
            },
        )?;
        let mut transactions = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let base_ms = base_started.elapsed().as_secs_f64() * 1000.0;
        let has_next = transactions.len() > limit;
        transactions.truncate(limit);
        let next = has_next.then(|| {
            let last = transactions
                .last()
                .expect("a next page requires a full page");
            AccountCursor {
                date: last.date.clone(),
                source_id: last.source_id.clone(),
            }
        });

        let other_started = Instant::now();
        if !transactions.is_empty() {
            let indexes: HashMap<_, _> = transactions
                .iter()
                .enumerate()
                .map(|(index, transaction)| (transaction.source_id.clone(), index))
                .collect();
            let placeholders = std::iter::repeat_n("?", transactions.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT postings.source_id, postings.id, accounts.id, accounts.name, postings.amount
                 FROM postings
                 LEFT JOIN accounts ON accounts.id = postings.account_id
                 WHERE postings.source_id IN ({placeholders})
                 ORDER BY postings.source_id, accounts.name, postings.id"
            );
            let values: Vec<SqlValue> = transactions
                .iter()
                .map(|transaction| SqlValue::Text(transaction.source_id.clone()))
                .collect();
            let mut other_statement = self.connection.prepare(&sql)?;
            let rows = other_statement.query_map(params_from_iter(values.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    OtherPosting {
                        posting_id: row.get(1)?,
                        account_id: row.get(2)?,
                        account_name: row.get(3)?,
                        amount: row.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (source_id, posting) = row?;
                if let Some(index) = indexes.get(&source_id)
                    && posting.posting_id != transactions[*index].posting_id
                {
                    transactions[*index].other_postings.push(posting);
                }
            }
        }
        crate::perf::log(
            "db.account_transactions",
            serde_json::json!({
                "account_id": account_id,
                "rows": transactions.len(),
                "limit": limit,
                "base_query_ms": base_ms,
                "other_postings_queries": usize::from(!transactions.is_empty()),
                "other_postings_ms": other_started.elapsed().as_secs_f64() * 1000.0,
                "total_ms": total_started.elapsed().as_secs_f64() * 1000.0,
            }),
        );
        Ok(AccountTransactionsPage { transactions, next })
    }

    fn get_account(&self, id: &str) -> Result<AccountRecord, DatabaseError> {
        self.connection
            .query_row(
                "WITH RECURSIVE descendants(id) AS (
                    SELECT ?1
                    UNION ALL
                    SELECT accounts.id
                    FROM descendants
                    JOIN accounts ON accounts.parent_id = descendants.id
                 )
                 SELECT
                    accounts.id,
                    accounts.parent_id,
                    accounts.external_id,
                    accounts.name,
                    accounts.imported_name,
                    accounts.type,
                    accounts.latest_external_transaction_id,
                    (SELECT COALESCE(SUM(postings.amount), 0)
                     FROM postings
                     WHERE postings.account_id IN (SELECT id FROM descendants)),
                    EXISTS(SELECT 1 FROM accounts AS child WHERE child.parent_id = accounts.id)
                 FROM accounts WHERE accounts.id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, bool>(8)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| DatabaseError::AccountNotFound(id.to_owned()))
            .and_then(
                |(
                    id,
                    parent_id,
                    external_id,
                    name,
                    imported_name,
                    account_type,
                    latest,
                    balance,
                    has_children,
                )| {
                    Ok(AccountRecord {
                        id,
                        parent_id,
                        external_id,
                        name,
                        imported_name,
                        account_type: AccountType::parse(&account_type)?,
                        latest_external_transaction_id: latest,
                        balance,
                        has_children,
                    })
                },
            )
    }
}

fn rebuild_projection(tx: &Transaction<'_>, source_id: &str) -> Result<(), DatabaseError> {
    let source = load_source(tx, source_id)?;
    let changes = load_changes(tx, source_id)?;
    let projection = reduce(source_id, &source.transaction, &changes)?;

    for posting in &projection.postings {
        if let Some(account_id) = &posting.posting.account_id {
            ensure_account(tx, account_id)?;
        }
    }
    tx.execute("DELETE FROM postings WHERE source_id = ?", [source_id])?;
    for posting in projection.postings {
        tx.execute(
            "INSERT INTO postings (
                id, source_id, account_id, date, payee, amount, pending, notes
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                posting.id,
                posting.source_id,
                posting.posting.account_id,
                posting.posting.date,
                posting.posting.payee,
                posting.posting.amount,
                posting.posting.pending,
                posting.posting.notes,
            ],
        )?;
    }
    Ok(())
}

fn upsert_source(tx: &Transaction<'_>, id: &str, input: &SourceInput) -> Result<(), DatabaseError> {
    crate::model::validate_source(id, input)?;
    if let Some(account_id) = &input.transaction.account_id {
        ensure_account(tx, account_id)?;
    }
    tx.execute(
        "INSERT INTO transactions_source (
            id, source, external_account_id, external_transaction_id, account_id, date,
            payee, amount, category, pending, opening_balance, notes, raw_json, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET
            source = excluded.source,
            external_account_id = excluded.external_account_id,
            external_transaction_id = excluded.external_transaction_id,
            account_id = excluded.account_id,
            date = excluded.date,
            payee = excluded.payee,
            amount = excluded.amount,
            category = excluded.category,
            pending = excluded.pending,
            opening_balance = excluded.opening_balance,
            notes = excluded.notes,
            raw_json = excluded.raw_json,
            updated_at = CURRENT_TIMESTAMP",
        params![
            id,
            input.source,
            input.external_account_id,
            input.external_transaction_id,
            input.transaction.account_id,
            input.transaction.date,
            input.transaction.payee,
            input.transaction.amount,
            input.transaction.category,
            input.transaction.pending,
            input.transaction.opening_balance,
            input.transaction.notes,
            serde_json::to_string(&input.raw)?,
        ],
    )?;
    Ok(())
}

fn imported_source(
    config: &ImportConfig,
    account: &AccountRecord,
    transaction: &ImportedTransaction,
) -> SourceInput {
    SourceInput {
        source: config.id.clone(),
        external_account_id: account.external_id.clone().unwrap_or_default(),
        external_transaction_id: transaction.id.clone(),
        transaction: SourceTransactionData {
            account_id: Some(account.id.clone()),
            date: transaction.date.clone(),
            payee: transaction.payee.clone(),
            amount: transaction.amount,
            category: transaction.category.clone(),
            pending: transaction.pending,
            opening_balance: transaction.opening_balance,
            notes: transaction.notes.clone(),
        },
        raw: transaction.raw.clone(),
    }
}

fn parse_namespace(id: &str) -> Result<Uuid, DatabaseError> {
    Uuid::parse_str(id)
        .map_err(|_| ModelError::InvalidUuid(id.to_owned()))
        .map_err(Into::into)
}

fn imported_uuid(namespace: &Uuid, kind: &str, external_id: &str) -> String {
    Uuid::new_v5(namespace, format!("{kind}:{external_id}").as_bytes()).to_string()
}

fn validate_external_id(id: &str) -> Result<(), DatabaseError> {
    if id.is_empty() {
        return Err(ModelError::InvalidExternalId.into());
    }
    Ok(())
}

fn validate_account_hierarchy(
    connection: &Connection,
    id: &str,
    parent_id: Option<&str>,
    account_type: AccountType,
) -> Result<(), DatabaseError> {
    if let Some(parent_id) = parent_id {
        let parent_type = connection
            .query_row(
                "SELECT type FROM accounts WHERE id = ?",
                [parent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| DatabaseError::AccountNotFound(parent_id.to_owned()))?;
        if AccountType::parse(&parent_type)? != account_type {
            return Err(ModelError::AccountTypeMismatch.into());
        }
        let creates_cycle = connection
            .query_row(
                "WITH RECURSIVE ancestors(id, parent_id) AS (
                    SELECT id, parent_id FROM accounts WHERE id = ?1
                    UNION
                    SELECT accounts.id, accounts.parent_id
                    FROM accounts
                    JOIN ancestors ON accounts.id = ancestors.parent_id
                 )
                 SELECT 1 FROM ancestors WHERE id = ?2 LIMIT 1",
                params![parent_id, id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if creates_cycle {
            return Err(ModelError::AccountCycle(id.to_owned()).into());
        }
    }

    let child_type_mismatch = connection
        .query_row(
            "SELECT 1 FROM accounts WHERE parent_id = ? AND type != ? LIMIT 1",
            params![id, account_type.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if child_type_mismatch {
        return Err(ModelError::AccountTypeMismatch.into());
    }
    Ok(())
}

fn ensure_source(connection: &Connection, id: &str) -> Result<(), DatabaseError> {
    connection
        .query_row(
            "SELECT 1 FROM transactions_source WHERE id = ?",
            [id],
            |_| Ok(()),
        )
        .optional()?
        .ok_or_else(|| DatabaseError::SourceNotFound(id.to_owned()))
}

fn ensure_account(connection: &Connection, id: &str) -> Result<(), DatabaseError> {
    connection
        .query_row("SELECT 1 FROM accounts WHERE id = ?", [id], |_| Ok(()))
        .optional()?
        .ok_or_else(|| ModelError::UnknownAccount(id.to_owned()).into())
}

fn descendant_account_ids(
    connection: &Connection,
    account_id: &str,
) -> Result<Vec<String>, DatabaseError> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE descendants(id) AS (
            SELECT ?1
            UNION ALL
            SELECT accounts.id
            FROM descendants
            JOIN accounts ON accounts.parent_id = descendants.id
         )
         SELECT id FROM descendants",
    )?;
    let rows = statement.query_map([account_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn delete_accounts(
    tx: &Transaction<'_>,
    account_ids: &[String],
    delete_transactions: bool,
) -> Result<AccountDeletion, DatabaseError> {
    let placeholders = std::iter::repeat_n("?", account_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let account_values: Vec<SqlValue> = account_ids.iter().cloned().map(SqlValue::Text).collect();
    let posting_sql =
        format!("SELECT id, source_id FROM postings WHERE account_id IN ({placeholders})");
    let posting_rows = {
        let mut statement = tx.prepare(&posting_sql)?;
        let rows = statement.query_map(params_from_iter(account_values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let affected_sources: HashSet<String> = posting_rows
        .iter()
        .map(|(_, source_id)| source_id.clone())
        .collect();

    let deleted_transactions = if delete_transactions {
        for source_id in &affected_sources {
            tx.execute("DELETE FROM transactions_source WHERE id = ?", [source_id])?;
        }
        affected_sources.len()
    } else {
        for (posting_id, source_id) in &posting_rows {
            tx.execute(
                "INSERT INTO transactions_changes (
                    source_id, operation, target_id, field, value_json
                 ) VALUES (?, 'set', ?, 'account_id', 'null')",
                params![source_id, posting_id],
            )?;
        }
        let source_sql = format!(
            "UPDATE transactions_source SET account_id = NULL WHERE account_id IN ({placeholders})"
        );
        tx.execute(&source_sql, params_from_iter(account_values.iter()))?;
        for source_id in &affected_sources {
            rebuild_projection(tx, source_id)?;
        }
        0
    };

    let delete_sql = format!("DELETE FROM accounts WHERE id IN ({placeholders})");
    tx.execute(&delete_sql, params_from_iter(account_values.iter()))?;
    Ok(AccountDeletion {
        accounts: account_ids.len(),
        transactions: deleted_transactions,
        nulled_postings: if delete_transactions {
            0
        } else {
            posting_rows.len()
        },
    })
}

fn load_source(connection: &Connection, id: &str) -> Result<SourceRecord, DatabaseError> {
    connection
        .query_row(
            "SELECT
                id, source, external_account_id, external_transaction_id, account_id, date,
                payee, amount, category, pending, opening_balance, notes, raw_json, updated_at
             FROM transactions_source
             WHERE id = ?",
            [id],
            |row| {
                let raw_json: String = row.get(12)?;
                Ok((
                    SourceRecord {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        external_account_id: row.get(2)?,
                        external_transaction_id: row.get(3)?,
                        transaction: SourceTransactionData {
                            account_id: row.get(4)?,
                            date: row.get(5)?,
                            payee: row.get(6)?,
                            amount: row.get(7)?,
                            category: row.get(8)?,
                            pending: row.get(9)?,
                            opening_balance: row.get(10)?,
                            notes: row.get(11)?,
                        },
                        raw: Value::Null,
                        updated_at: row.get(13)?,
                    },
                    raw_json,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| DatabaseError::SourceNotFound(id.to_owned()))
        .and_then(|(mut source, raw_json)| {
            source.raw = serde_json::from_str(&raw_json)?;
            Ok(source)
        })
}

fn load_changes(
    connection: &Connection,
    source_id: &str,
) -> Result<Vec<StoredChange>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT id, operation, target_id, field, value_json, created_at
         FROM transactions_changes
         WHERE source_id = ?
         ORDER BY id",
    )?;
    let rows = statement.query_map([source_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    rows.map(|row| {
        let (id, operation, target_id, field, value_json, created_at) = row?;
        Ok(StoredChange {
            id,
            change: decode_change(&operation, target_id, field, value_json)?,
            created_at,
        })
    })
    .collect()
}

fn load_postings(connection: &Connection, source_id: &str) -> Result<Vec<Posting>, DatabaseError> {
    let mut statement = connection.prepare(
        "SELECT id, source_id, account_id, date, payee, amount, pending, notes
         FROM postings
         WHERE source_id = ?
         ORDER BY CASE WHEN id = source_id THEN 0 ELSE 1 END, id",
    )?;
    let rows = statement.query_map([source_id], |row| {
        Ok(Posting {
            id: row.get(0)?,
            source_id: row.get(1)?,
            posting: PostingData {
                account_id: row.get(2)?,
                date: row.get(3)?,
                payee: row.get(4)?,
                amount: row.get(5)?,
                pending: row.get(6)?,
                notes: row.get(7)?,
            },
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn encode_change(change: &Change) -> Result<EncodedChange<'_>, DatabaseError> {
    Ok(match change {
        Change::Set {
            target_id,
            field,
            value,
        } => (
            "set",
            target_id,
            Some(field),
            (!value.is_null())
                .then(|| serde_json::to_string(value))
                .transpose()?,
        ),
        Change::Split { target_id } => ("split", target_id, None, None),
        Change::RemoveSplit { target_id } => ("remove_split", target_id, None, None),
    })
}

fn decode_change(
    operation: &str,
    target_id: String,
    field: Option<String>,
    value_json: Option<String>,
) -> Result<Change, DatabaseError> {
    match operation {
        "set" => Ok(Change::Set {
            target_id,
            field: field.ok_or_else(|| ModelError::UnknownField("NULL".into()))?,
            value: value_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?
                .unwrap_or(Value::Null),
        }),
        "split" => Ok(Change::Split { target_id }),
        "remove_split" => Ok(Change::RemoveSplit { target_id }),
        other => Err(ModelError::UnknownField(format!("operation:{other}")).into()),
    }
}
