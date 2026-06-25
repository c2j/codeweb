-- Regression: function call in PL/pgSQL assignment expression
-- Bug: visit_expr was missing on CallExtractor, so v := calc_total(1)
-- produced no DirectCall edge. Fixed in v0.7.3.

CREATE FUNCTION calc_total(p INT) RETURNS INTEGER AS $$
BEGIN
    RETURN p * 2;
END;
$$;

CREATE PROCEDURE process_order AS $$
DECLARE
    v_total INTEGER;
BEGIN
    v_total := calc_total(1);
END;
$$;
