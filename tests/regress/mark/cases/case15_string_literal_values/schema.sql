CREATE TABLE orders (
    id INT PRIMARY KEY,
    amount NUMERIC(10, 2)
);

CREATE TABLE audit_log (
    id INT PRIMARY KEY,
    table_name VARCHAR(100),
    action VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE process_orders()
AS
BEGIN
    INSERT INTO audit_log (table_name, action) VALUES ('orders', 'processed');
    INSERT INTO orders (id, amount) VALUES (0, 0);
END;
/
