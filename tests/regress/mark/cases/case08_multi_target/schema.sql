-- Case 08: Multi-target — --node accounts --node inventory
-- Reuses case03 schema

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

CREATE OR REPLACE PROCEDURE check_inventory()
AS
BEGIN
    SELECT * FROM inventory WHERE quantity < 10;
END;
/
