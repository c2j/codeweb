-- Case 14: Fingerprint matching — indirect match via body_sql
-- Target: table `accounts`
--   caller: proc `verify_balance` which accesses accounts via TableAccess
--   verify_balance body_sql: "SELECT SUM(balance) INTO v_total FROM accounts WHERE owner = 'ACTIVE'"
-- Expected:
--   - WDR with SQL that references accounts → direct match (table name)
--   - WDR with SQL that calls verify_balance → indirect match (name)
--   - WDR with SQL that fingerprint-matches verify_balance body → indirect match (fingerprint)
--   - WDR with unrelated SQL → no match

CREATE TABLE accounts (
    id INT PRIMARY KEY,
    balance NUMERIC(15, 2),
    owner VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE verify_balance()
AS
    v_total NUMERIC(15, 2);
BEGIN
    SELECT SUM(balance) INTO v_total FROM accounts WHERE owner = 'ACTIVE';
END;
/
