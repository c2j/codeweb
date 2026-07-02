-- Case 09: DML statements must NOT falsely match procedure/function names
-- Target: table `orders`
--     caller: proc `sync_orders` (accesses orders via TableAccess)
-- Expected: regular DML with "sync_orders" as a substring should NOT match

CREATE TABLE orders (
    id INT PRIMARY KEY,
    amount NUMERIC(10, 2)
);

CREATE OR REPLACE PROCEDURE sync_orders()
AS
BEGIN
    INSERT INTO orders (id, amount) VALUES (0, 0);
END;
/
