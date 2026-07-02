-- Case 03: Table with deep caller-chain (3-level) + fingerprint match
-- Target: table `accounts`
-- Graph:
--   accounts ←TableAccess-- audit_accounts ←DirectCall-- monthly_close ←DirectCall-- year_end
--   accounts ←TableAccess-- verify_balance
--
-- Also: verify_balance has body_sql that can be fingerprint-matched

CREATE TABLE accounts (
    id INT PRIMARY KEY,
    balance NUMERIC(15, 2),
    owner VARCHAR(100)
);

CREATE TABLE inventory (
    id INT PRIMARY KEY,
    name VARCHAR(100),
    quantity INT
);

CREATE OR REPLACE PROCEDURE audit_accounts()
AS
BEGIN
    INSERT INTO accounts (id, balance, owner) VALUES (-1, 0, 'AUDIT');
END;
/

CREATE OR REPLACE PROCEDURE monthly_close()
AS
BEGIN
    CALL audit_accounts();
END;
/

CREATE OR REPLACE PROCEDURE year_end()
AS
BEGIN
    CALL monthly_close();
END;
/

CREATE OR REPLACE PROCEDURE verify_balance()
AS
    v_total NUMERIC(15, 2);
BEGIN
    SELECT SUM(balance) INTO v_total FROM accounts WHERE owner = 'ACTIVE';
END;
/
