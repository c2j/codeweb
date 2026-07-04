-- Case 13: Jaccard false positive — same table, different columns/conditions
-- Target: table `accounts`
--   caller: proc `summarize_accounts` body_sql = "SELECT id, balance FROM accounts WHERE owner = 'ACTIVE'"
-- Expected:
--   - WDR with different columns and conditions but same table → should NOT be fingerprint-matched
--     unless it contains "accounts" directly (direct match)
--   - The Jaccard threshold of 0.8 can produce false positives for short/simple SQL

CREATE TABLE accounts (
    id INT PRIMARY KEY,
    balance NUMERIC(15, 2),
    owner VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE summarize_accounts()
AS
    v_total NUMERIC(15, 2);
BEGIN
    SELECT SUM(balance) INTO v_total FROM accounts WHERE owner = 'ACTIVE';
END;
/

