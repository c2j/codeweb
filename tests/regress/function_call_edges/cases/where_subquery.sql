-- Regression: function call inside subquery in WHERE clause
-- Verifies walker descends into nested subquery expressions:
-- walk_select → walk_expr(WHERE) → walk_select(subquery) → walk_expr(target)

CREATE FUNCTION get_threshold(p_type VARCHAR) RETURNS INTEGER AS $$
BEGIN
    RETURN CASE p_type WHEN 'VIP' THEN 100 ELSE 10 END;
END;
$$ LANGUAGE plpgsql;

CREATE TABLE orders (
    id INT,
    amount NUMERIC,
    cust_type VARCHAR
);

CREATE PROCEDURE find_high_value_orders AS $$
DECLARE
    v_id INT;
BEGIN
    SELECT id INTO v_id FROM orders
    WHERE amount > (
        SELECT get_threshold(cust_type) FROM orders WHERE id = 1
    );
END;
$$;
