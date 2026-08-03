PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE transactions_source_new (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_account_id TEXT NOT NULL,
    external_transaction_id TEXT NOT NULL,
    account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
    date TEXT NOT NULL,
    payee TEXT,
    amount INTEGER NOT NULL,
    category TEXT,
    pending INTEGER NOT NULL DEFAULT 0 CHECK (pending IN (0, 1)),
    opening_balance INTEGER NOT NULL DEFAULT 0 CHECK (opening_balance IN (0, 1)),
    notes TEXT,
    raw_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (source, external_account_id, external_transaction_id)
);

INSERT INTO transactions_source_new
SELECT * FROM transactions_source;

DROP TABLE transactions_source;
ALTER TABLE transactions_source_new RENAME TO transactions_source;

CREATE TABLE postings_new (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES transactions_source(id) ON DELETE CASCADE,
    account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
    date TEXT NOT NULL,
    payee TEXT,
    amount INTEGER NOT NULL,
    pending INTEGER NOT NULL CHECK (pending IN (0, 1)),
    notes TEXT
);

INSERT INTO postings_new
SELECT * FROM postings;

DROP TABLE postings;
ALTER TABLE postings_new RENAME TO postings;

CREATE INDEX postings_source_id ON postings(source_id);
CREATE UNIQUE INDEX postings_source_account ON postings(source_id, account_id);
CREATE INDEX postings_account_date_source ON postings(account_id, date DESC, source_id DESC);

COMMIT;

PRAGMA foreign_keys = ON;
