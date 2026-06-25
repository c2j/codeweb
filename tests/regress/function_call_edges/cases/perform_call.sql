-- Regression: function call via PERFORM statement
-- Bug: same root cause as expr_assignment. PERFORM bar() was walked
-- but Expr::FunctionCall was not captured. Fixed in v0.7.3.

CREATE FUNCTION bar() RETURNS INTEGER AS $$
BEGIN
    RETURN 1;
END;
$$;

CREATE PROCEDURE foo() AS $$
BEGIN
    PERFORM bar();
END;
$$;
