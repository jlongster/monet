PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS import_config (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    id TEXT NOT NULL,
    url TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES accounts(id),
    external_id TEXT UNIQUE,
    name TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('asset', 'liability', 'income', 'expense', 'equity')),
    latest_external_transaction_id TEXT
);

CREATE INDEX IF NOT EXISTS accounts_parent_id
ON accounts(parent_id);

CREATE TABLE IF NOT EXISTS transactions_source (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_account_id TEXT NOT NULL,
    external_transaction_id TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    date TEXT NOT NULL,
    payee TEXT,
    amount INTEGER NOT NULL,
    category TEXT,
    pending INTEGER NOT NULL DEFAULT 0 CHECK (pending IN (0, 1)),
    notes TEXT,
    raw_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (source, external_account_id, external_transaction_id)
);

CREATE TABLE IF NOT EXISTS transactions_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id TEXT NOT NULL REFERENCES transactions_source(id) ON DELETE CASCADE,
    operation TEXT NOT NULL CHECK (operation IN ('set', 'split', 'remove_split')),
    target_id TEXT NOT NULL,
    field TEXT,
    value_json TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS transactions_changes_source_id_id
ON transactions_changes(source_id, id);

CREATE TABLE IF NOT EXISTS postings (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES transactions_source(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(id),
    date TEXT NOT NULL,
    payee TEXT,
    amount INTEGER NOT NULL,
    pending INTEGER NOT NULL CHECK (pending IN (0, 1)),
    notes TEXT
);

CREATE INDEX IF NOT EXISTS postings_source_id
ON postings(source_id);

CREATE UNIQUE INDEX IF NOT EXISTS postings_source_account
ON postings(source_id, account_id);

CREATE INDEX IF NOT EXISTS postings_account_date_source
ON postings(account_id, date DESC, source_id DESC);
