use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    Asset,
    Liability,
    Income,
    Expense,
    Equity,
}

impl AccountType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asset => "asset",
            Self::Liability => "liability",
            Self::Income => "income",
            Self::Expense => "expense",
            Self::Equity => "equity",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ModelError> {
        match value {
            "asset" => Ok(Self::Asset),
            "liability" => Ok(Self::Liability),
            "income" => Ok(Self::Income),
            "expense" => Ok(Self::Expense),
            "equity" => Ok(Self::Equity),
            _ => Err(ModelError::InvalidAccountType(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccountInput {
    pub parent_id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: AccountType,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub external_id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: AccountType,
    pub latest_external_transaction_id: Option<String>,
    pub balance: i64,
    pub has_children: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ImportConfigInput {
    pub url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportConfig {
    pub id: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportedAccount {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: AccountType,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportedAccounts {
    pub accounts: Vec<ImportedAccount>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportedTransaction {
    pub id: String,
    pub date: String,
    pub payee: Option<String>,
    pub amount: i64,
    pub category: Option<String>,
    #[serde(default)]
    pub pending: bool,
    pub notes: Option<String>,
    #[serde(default)]
    pub raw: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ImportedTransactions {
    pub transactions: Vec<ImportedTransaction>,
    #[serde(default)]
    pub pending_ids: Vec<String>,
    pub latest: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportSummary {
    pub accounts: usize,
    pub transactions: usize,
    pub deleted_pending: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SourceTransactionData {
    pub account_id: String,
    pub date: String,
    pub payee: Option<String>,
    pub amount: i64,
    pub category: Option<String>,
    #[serde(default)]
    pub pending: bool,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SourceInput {
    pub source: String,
    pub external_account_id: String,
    pub external_transaction_id: String,
    #[serde(flatten)]
    pub transaction: SourceTransactionData,
    #[serde(default)]
    pub raw: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceRecord {
    pub id: String,
    pub source: String,
    pub external_account_id: String,
    pub external_transaction_id: String,
    #[serde(flatten)]
    pub transaction: SourceTransactionData,
    pub raw: Value,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Change {
    Set {
        target_id: String,
        field: String,
        value: Value,
    },
    Split {
        target_id: String,
    },
    RemoveSplit {
        target_id: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
pub struct ChangesInput {
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoredChange {
    pub id: i64,
    #[serde(flatten)]
    pub change: Change,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PostingData {
    pub account_id: String,
    pub date: String,
    pub payee: Option<String>,
    pub amount: i64,
    pub pending: bool,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Posting {
    pub id: String,
    pub source_id: String,
    #[serde(flatten)]
    pub posting: PostingData,
}

#[derive(Clone, Debug, Serialize)]
pub struct Projection {
    pub postings: Vec<Posting>,
    pub unallocated_amount: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceBundle {
    pub source: SourceRecord,
    pub changes: Vec<StoredChange>,
    #[serde(flatten)]
    pub projection: Projection,
}

#[derive(Clone, Debug, Serialize)]
pub struct OtherPosting {
    pub posting_id: String,
    pub account_id: String,
    pub account_name: String,
    pub amount: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountTransaction {
    pub source_id: String,
    pub account_id: String,
    pub date: String,
    pub payee: Option<String>,
    pub amount: i64,
    pub pending: bool,
    pub notes: Option<String>,
    pub posting_count: i64,
    pub unallocated_amount: i64,
    pub other_postings: Vec<OtherPosting>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountCursor {
    pub date: String,
    pub source_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountTransactionsPage {
    pub transactions: Vec<AccountTransaction>,
    pub next: Option<AccountCursor>,
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid UUID: {0}")]
    InvalidUuid(String),
    #[error("invalid account type: {0}")]
    InvalidAccountType(String),
    #[error("account name must not be empty")]
    InvalidAccountName,
    #[error("account hierarchy contains a cycle involving '{0}'")]
    AccountCycle(String),
    #[error("parent and child accounts must have the same type")]
    AccountTypeMismatch,
    #[error("external ID must not be empty")]
    InvalidExternalId,
    #[error("unknown field '{0}'")]
    UnknownField(String),
    #[error("field '{field}' requires {expected}")]
    InvalidValue {
        field: String,
        expected: &'static str,
    },
    #[error("split '{0}' already exists")]
    DuplicateSplit(String),
    #[error("split '{0}' does not exist")]
    UnknownSplit(String),
    #[error("split '{0}' has no account")]
    MissingSplitAccount(String),
    #[error("split '{0}' has no amount")]
    MissingSplitAmount(String),
    #[error("source contains more than one posting for account '{0}'")]
    DuplicatePostingAccount(String),
    #[error("account '{0}' does not exist")]
    UnknownAccount(String),
    #[error("source '{0}' is not pending")]
    SourceNotPending(String),
    #[error("transaction amount overflow")]
    AmountOverflow,
    #[error("source date must not be empty")]
    InvalidSource,
}

#[derive(Clone, Debug)]
struct Split {
    id: String,
    account_id: Option<String>,
    amount: Option<i64>,
}

pub fn validate_account(id: &str, account: &AccountInput) -> Result<(), ModelError> {
    validate_uuid(id)?;
    if let Some(parent_id) = &account.parent_id {
        validate_uuid(parent_id)?;
        if parent_id == id {
            return Err(ModelError::InvalidValue {
                field: "parent_id".into(),
                expected: "a different account UUID",
            });
        }
    }
    if account.name.trim().is_empty() {
        return Err(ModelError::InvalidAccountName);
    }
    Ok(())
}

pub fn validate_source(id: &str, source: &SourceInput) -> Result<(), ModelError> {
    validate_uuid(id)?;
    validate_uuid(&source.transaction.account_id)?;
    if source.transaction.date.is_empty() {
        return Err(ModelError::InvalidSource);
    }
    Ok(())
}

pub fn reduce(
    source_id: &str,
    source: &SourceTransactionData,
    changes: &[StoredChange],
) -> Result<Projection, ModelError> {
    let mut base = PostingData {
        account_id: source.account_id.clone(),
        date: source.date.clone(),
        payee: source.payee.clone(),
        amount: source.amount,
        pending: source.pending,
        notes: source.notes.clone(),
    };
    let mut splits: Vec<Split> = Vec::new();
    let mut used_split_ids = HashSet::new();

    for stored in changes {
        match &stored.change {
            Change::Set {
                target_id,
                field,
                value,
            } if target_id == source_id => set_base(&mut base, field, value)?,
            Change::Set {
                target_id,
                field,
                value,
            } => {
                let split = splits
                    .iter_mut()
                    .find(|split| split.id == *target_id)
                    .ok_or_else(|| ModelError::UnknownSplit(target_id.clone()))?;
                set_split(split, field, value)?;
            }
            Change::Split { target_id } => {
                validate_uuid(target_id)?;
                if target_id == source_id || !used_split_ids.insert(target_id.clone()) {
                    return Err(ModelError::DuplicateSplit(target_id.clone()));
                }
                splits.push(Split {
                    id: target_id.clone(),
                    account_id: None,
                    amount: None,
                });
            }
            Change::RemoveSplit { target_id } => {
                let index = splits
                    .iter()
                    .position(|split| split.id == *target_id)
                    .ok_or_else(|| ModelError::UnknownSplit(target_id.clone()))?;
                splits.remove(index);
            }
        }
    }

    let mut allocated = base.amount;
    let mut posting_accounts = HashSet::from([base.account_id.clone()]);
    let mut postings = Vec::with_capacity(splits.len() + 1);
    postings.push(Posting {
        id: source_id.to_owned(),
        source_id: source_id.to_owned(),
        posting: base.clone(),
    });

    for split in splits {
        let account_id = split
            .account_id
            .ok_or_else(|| ModelError::MissingSplitAccount(split.id.clone()))?;
        let amount = split
            .amount
            .ok_or_else(|| ModelError::MissingSplitAmount(split.id.clone()))?;
        if !posting_accounts.insert(account_id.clone()) {
            return Err(ModelError::DuplicatePostingAccount(account_id));
        }
        allocated = allocated
            .checked_add(amount)
            .ok_or(ModelError::AmountOverflow)?;

        let mut posting = base.clone();
        posting.account_id = account_id;
        posting.amount = amount;
        postings.push(Posting {
            id: split.id,
            source_id: source_id.to_owned(),
            posting,
        });
    }

    Ok(Projection {
        postings,
        unallocated_amount: allocated.checked_neg().ok_or(ModelError::AmountOverflow)?,
    })
}

fn set_base(base: &mut PostingData, field: &str, value: &Value) -> Result<(), ModelError> {
    match field {
        "account_id" => base.account_id = required_uuid(field, value)?,
        "date" => base.date = required_string(field, value)?,
        "payee" => base.payee = optional_string(field, value)?,
        "amount" => base.amount = integer(field, value)?,
        "notes" => base.notes = optional_string(field, value)?,
        _ => return Err(ModelError::UnknownField(field.to_owned())),
    }
    Ok(())
}

fn set_split(split: &mut Split, field: &str, value: &Value) -> Result<(), ModelError> {
    match field {
        "account_id" => split.account_id = Some(required_uuid(field, value)?),
        "amount" => split.amount = Some(integer(field, value)?),
        _ => return Err(ModelError::UnknownField(field.to_owned())),
    }
    Ok(())
}

fn validate_uuid(value: &str) -> Result<(), ModelError> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| ModelError::InvalidUuid(value.to_owned()))
}

fn required_uuid(field: &str, value: &Value) -> Result<String, ModelError> {
    let value = required_string(field, value)?;
    validate_uuid(&value)?;
    Ok(value)
}

fn required_string(field: &str, value: &Value) -> Result<String, ModelError> {
    value
        .as_str()
        .map(str::to_owned)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ModelError::InvalidValue {
            field: field.to_owned(),
            expected: "a non-empty string",
        })
}

fn optional_string(field: &str, value: &Value) -> Result<Option<String>, ModelError> {
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| ModelError::InvalidValue {
            field: field.to_owned(),
            expected: "a string or null",
        })
}

fn integer(field: &str, value: &Value) -> Result<i64, ModelError> {
    value.as_i64().ok_or_else(|| ModelError::InvalidValue {
        field: field.to_owned(),
        expected: "an integer",
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    const SOURCE_ID: &str = "019bf123-0000-7000-8000-000000000001";
    const CHECKING_ID: &str = "019bf123-0000-7000-8000-000000000010";
    const GROCERIES_ID: &str = "019bf123-0000-7000-8000-000000000011";
    const POSTING_ID: &str = "019bf123-0000-7000-8000-000000000002";

    fn source(amount: i64) -> SourceTransactionData {
        SourceTransactionData {
            account_id: CHECKING_ID.into(),
            date: "2026-07-20".into(),
            payee: Some("TARGET #1842".into()),
            amount,
            category: Some("Shopping".into()),
            pending: false,
            notes: None,
        }
    }

    fn stored(id: i64, change: Change) -> StoredChange {
        StoredChange {
            id,
            change,
            created_at: String::new(),
        }
    }

    #[test]
    fn emits_the_known_source_posting_when_unassigned() {
        let projection = reduce(SOURCE_ID, &source(-10_000), &[]).unwrap();

        assert_eq!(projection.postings.len(), 1);
        assert_eq!(projection.postings[0].id, SOURCE_ID);
        assert_eq!(projection.postings[0].posting.account_id, CHECKING_ID);
        assert_eq!(projection.postings[0].posting.amount, -10_000);
        assert_eq!(projection.unallocated_amount, 10_000);
    }

    #[test]
    fn source_changes_are_copied_to_every_posting() {
        let changes = vec![
            stored(
                1,
                Change::Set {
                    target_id: SOURCE_ID.into(),
                    field: "payee".into(),
                    value: json!("Target"),
                },
            ),
            stored(
                2,
                Change::Split {
                    target_id: POSTING_ID.into(),
                },
            ),
            stored(
                3,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "account_id".into(),
                    value: json!(GROCERIES_ID),
                },
            ),
            stored(
                4,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "amount".into(),
                    value: json!(10_000),
                },
            ),
        ];

        let projection = reduce(SOURCE_ID, &source(-10_000), &changes).unwrap();
        assert_eq!(projection.postings.len(), 2);
        assert_eq!(projection.unallocated_amount, 0);
        assert!(
            projection
                .postings
                .iter()
                .all(|posting| posting.posting.payee.as_deref() == Some("Target"))
        );
    }

    #[test]
    fn partial_splits_leave_a_derived_unallocated_amount() {
        let changes = vec![
            stored(
                1,
                Change::Split {
                    target_id: POSTING_ID.into(),
                },
            ),
            stored(
                2,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "account_id".into(),
                    value: json!(GROCERIES_ID),
                },
            ),
            stored(
                3,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "amount".into(),
                    value: json!(6000),
                },
            ),
        ];

        let projection = reduce(SOURCE_ID, &source(-10_000), &changes).unwrap();
        assert_eq!(projection.postings.len(), 2);
        assert_eq!(projection.unallocated_amount, 4000);
    }

    #[test]
    fn source_amount_changes_recalculate_unallocated_amount() {
        let changes = vec![
            stored(
                1,
                Change::Split {
                    target_id: POSTING_ID.into(),
                },
            ),
            stored(
                2,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "account_id".into(),
                    value: json!(GROCERIES_ID),
                },
            ),
            stored(
                3,
                Change::Set {
                    target_id: POSTING_ID.into(),
                    field: "amount".into(),
                    value: json!(6000),
                },
            ),
        ];

        assert_eq!(
            reduce(SOURCE_ID, &source(-10_000), &changes)
                .unwrap()
                .unallocated_amount,
            4000
        );
        assert_eq!(
            reduce(SOURCE_ID, &source(-12_000), &changes)
                .unwrap()
                .unallocated_amount,
            6000
        );
    }
}
