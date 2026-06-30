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

-- Case 4: Recursive CTE (self-reference in CTE body)
CREATE PROCEDURE process_recursive() AS $$
DECLARE
    v_count INT;
BEGIN
    WITH RECURSIVE rec_cte AS (
        SELECT id, customer_id FROM orders WHERE id = 1
        UNION ALL
        SELECT o.id, o.customer_id
        FROM orders o
        JOIN rec_cte r ON o.id = r.id + 1
    )
    SELECT COUNT(*) INTO v_count FROM rec_cte;
END;
$$;

-- Case 5: INSERT WITH CTE
CREATE TABLE audit_log (order_id INT, total DECIMAL);

CREATE PROCEDURE process_insert_with_cte() AS $$
BEGIN
    WITH ins_cte AS (
        SELECT id, amount FROM orders WHERE amount > 100
    )
    INSERT INTO audit_log (order_id, total)
    SELECT id, amount FROM ins_cte;
END;
$$;
