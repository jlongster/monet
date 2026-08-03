ALTER TABLE accounts ADD COLUMN imported_name TEXT;

UPDATE accounts
SET imported_name = name
WHERE external_id IS NOT NULL;
