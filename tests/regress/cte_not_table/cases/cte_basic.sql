-- Regression: CTE names must not be recognized as database table nodes
-- Bug: WITH clause CTE table references may be incorrectly treated as physical tables
-- in the TableAccessExtractor, creating spurious "table" type nodes.

CREATE TABLE orders (
    id INT,
    customer_id INT,
    amount DECIMAL
);

CREATE TABLE customers (
    id INT,
    name VARCHAR
);

-- Case 1: Basic WITH clause CTE
CREATE PROCEDURE process_orders() AS $$
DECLARE
    v_total DECIMAL;
BEGIN
    WITH cte_orders AS (
        SELECT id, amount FROM orders WHERE amount > 100
    )
    SELECT SUM(amount) INTO v_total FROM cte_orders;
END;
$$;

-- Case 2: Multiple CTEs
CREATE PROCEDURE process_multiple() AS $$
DECLARE
    v_count INT;
BEGIN
    WITH
        cte_orders AS (
            SELECT id, customer_id FROM orders
        ),
        cte_customers AS (
            SELECT id, name FROM customers
        )
    SELECT COUNT(*) INTO v_count
    FROM cte_orders o
    JOIN cte_customers c ON o.customer_id = c.id;
END;
$$;

-- Case 3: CTE used in JOIN
CREATE PROCEDURE process_joined() AS $$
DECLARE
    v_total DECIMAL;
BEGIN
    WITH cte_joined AS (
        SELECT o.id, o.amount, c.name
        FROM orders o
        JOIN customers c ON o.customer_id = c.id
    )
    SELECT SUM(amount) INTO v_total FROM cte_joined;
END;
$$;
