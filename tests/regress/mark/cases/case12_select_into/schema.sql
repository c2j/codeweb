-- Case 12: SELECT INTO fingerprint — body_sql has INTO, WDR sql_text doesn't
-- Target: table `accounts`
--   caller: proc `verify_balance` body_sql = "SELECT SUM(balance) INTO v FROM accounts WHERE owner = 'X'"
--   WDR:       "SELECT SUM(balance) FROM accounts WHERE owner = 'X'"
-- Expected: fingerprint match even though INTO is missing in WDR

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
