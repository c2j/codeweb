-- Case 11: Fingerprint boundaries — similar SQL must NOT falsely match via fingerprint
-- Target: table `accounts`
--   caller: proc `audit_accounts` body_sql = "INSERT INTO accounts (id, balance, owner) VALUES (-1, 0, 'AUDIT')"
-- Expected:
--   - Same table, different values → direct match (table name), NOT fingerprint indirect
--   - Different table, same structure → NEITHER (no direct, no fingerprint)

CREATE TABLE accounts (
    id INT PRIMARY KEY,
    balance NUMERIC(15, 2),
    owner VARCHAR(100)
);

CREATE TABLE accounts_backup (
    id INT PRIMARY KEY,
    balance NUMERIC(15, 2),
    owner VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE audit_accounts()
AS
BEGIN
    INSERT INTO accounts (id, balance, owner) VALUES (-1, 0, 'AUDIT');
END;
/
