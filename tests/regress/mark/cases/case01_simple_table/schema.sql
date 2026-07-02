-- Case 01: Simple Table - Direct name match + Indirect caller match
-- Target: table `orders`
-- Graph: orders ←TableAccess-- update_orders, get_orders

CREATE TABLE orders (
    id INT PRIMARY KEY,
    amount NUMERIC(10, 2),
    status VARCHAR(20)
);

CREATE OR REPLACE PROCEDURE update_orders()
AS
BEGIN
    UPDATE orders SET status = 'processed' WHERE status = 'pending';
    INSERT INTO orders (id, amount, status) VALUES (0, 0, 'init');
END;
/

CREATE OR REPLACE PROCEDURE get_orders()
AS
    v_count INT;
BEGIN
    SELECT COUNT(*) INTO v_count FROM orders WHERE status = 'active';
END;
/
