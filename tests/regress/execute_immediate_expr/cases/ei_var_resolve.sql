-- Test: Variable content resolution for EXECUTE IMMEDIATE
--
-- Pattern: `v_sql := 'CALL target_proc()'; EXECUTE IMMEDIATE v_sql;`
-- With the fix, the CallExtractor tracks variable assignments and resolves
-- the CALL target inside the assigned string literal.
--
-- Expected:
--   - DirectCall edge from test_var_resolve -> target_proc
--     (from variable resolution, NOT from a direct CALL statement)
--   - 0 Unresolved nodes related to variable resolution
--   - Works for both bare variable and parenthesized variable

CREATE OR REPLACE PROCEDURE target_proc AS $$
BEGIN
    NULL;
END;
$$;

CREATE OR REPLACE PROCEDURE test_var_resolve AS $$
DECLARE
    v_sql VARCHAR2(10000);
BEGIN
    -- Pattern 1: Bare variable after assignment
    v_sql := 'CALL target_proc()';
    EXECUTE IMMEDIATE v_sql;

    -- Pattern 2: Parenthesized variable after assignment
    v_sql := 'CALL target_proc()';
    EXECUTE IMMEDIATE (v_sql);
END;
$$;
