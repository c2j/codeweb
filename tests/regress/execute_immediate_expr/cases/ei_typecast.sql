-- Test: EXECUTE IMMEDIATE with TypeCast expression (TypeCast escape test)
--
-- Note: This is an unusual pattern — CAST() wrapping a variable used as
-- EXECUTE IMMEDIATE target. In GaussDB this is typically not needed since
-- VARCHAR2 variables can be used directly. However, it tests whether
-- TypeCast escapes noise_rule.
--
-- noise_rule check: `TypeCast {` is NOT in the hardcoded prefix list → escapes.
--
-- Expected after fix: No Unresolved node whose raw_expr contains "TypeCast".

CREATE OR REPLACE PROCEDURE test_ei_typecast AS $$
DECLARE
    v_sql VARCHAR2(10000) := 'UPDATE t SET x = 1';
BEGIN
    EXECUTE IMMEDIATE CAST(v_sql AS VARCHAR2);
END;
$$;
