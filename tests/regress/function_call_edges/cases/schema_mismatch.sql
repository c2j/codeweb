-- Regression: schema-qualified function definition, unqualified call
-- Verifies two-phase resolution: create_edges creates Unresolved node,
-- resolve_unresolved_nodes resolves via bare_name_lower match.

CREATE SCHEMA biz;

CREATE FUNCTION biz.calc_total(p_qty INT, p_price NUMERIC) RETURNS NUMERIC AS $$
BEGIN
    RETURN p_qty * p_price;
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE process_order AS $$
DECLARE
    v_total NUMERIC;
BEGIN
    v_total := calc_total(10, 99.50);
END;
$$;
