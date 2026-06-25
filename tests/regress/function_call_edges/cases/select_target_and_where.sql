-- Regression: function calls in SELECT targets and WHERE clauses
-- Bug: walk_select walks these positions via walk_expr, but CallExtractor
-- had no visit_expr override. Fixed in v0.7.3.

CREATE FUNCTION format_name(p_first VARCHAR, p_last VARCHAR) RETURNS VARCHAR AS $$
BEGIN
    RETURN p_first || ' ' || p_last;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION get_priority(p_code VARCHAR) RETURNS INTEGER AS $$
BEGIN
    RETURN CASE p_code WHEN 'A' THEN 1 ELSE 0 END;
END;
$$ LANGUAGE plpgsql;

CREATE TABLE users (
    id INT,
    first_name VARCHAR,
    last_name VARCHAR,
    code VARCHAR
);

CREATE PROCEDURE report_users AS $$
DECLARE
    v_display VARCHAR;
    v_level INTEGER;
BEGIN
    SELECT format_name(first_name, last_name) INTO v_display FROM users WHERE id = 1;

    SELECT id INTO v_level FROM users WHERE get_priority(code) > 0;
END;
$$;
