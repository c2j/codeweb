CREATE TABLE accounts (
    id INT PRIMARY KEY,
    balance NUMERIC
);

CREATE TABLE audit_log (
    id INT PRIMARY KEY,
    table_name VARCHAR(100),
    action VARCHAR(100)
);

CREATE TABLE inventory (
    id INT PRIMARY KEY,
    name VARCHAR(100),
    qty INT
);

CREATE OR REPLACE PROCEDURE audit_accounts()
AS
BEGIN
    INSERT INTO audit_log (table_name, action) VALUES ('accounts', 'audited');
    UPDATE accounts SET balance = 0 WHERE id = 0;
END;
/

CREATE OR REPLACE PROCEDURE check_inventory()
AS
BEGIN
    INSERT INTO audit_log (table_name, action) VALUES ('inventory', 'checked');
END;
/
