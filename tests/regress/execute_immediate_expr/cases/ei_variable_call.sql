-- Test: EXECUTE IMMEDIATE with variable CALL (hidden relationship loss)
--
-- Pattern: Dynamic SQL via variable that CONTAINS a CALL to another procedure.
-- ogsql-parser cannot statically know the content of v_sql at execution time,
-- so parsed_query = None. This means the CALL inside the dynamic SQL is
-- INVISIBLE to the graph — the edge from test_ei_variable_call -> other_proc
-- is MISSING.
--
-- This is a FUNDAMENTAL LIMITATION of static analysis for dynamic SQL, not a bug.
-- But the current code makes it WORSE by:
--   1. Creating a garbage Unresolved node with Debug-format callee_name
--   2. Creating a DynamicCall edge to the garbage node
--   3. Storing a Debug string in body_sql that pollutes search results
--
-- Expected (current behavior):
--   - Unresolved node with raw_expr containing "PlVariable(["v_sql"])" (caught by noise_rule)
--     OR worse: if there's another Expr wrapper, it escapes
--   - NO DirectCall edge to other_proc via the EXECUTE IMMEDIATE variable
--   - TableAccess from other_proc -> t_archive exists (from other_proc's definition)
--   - But the PATH outer_proc -> ... -> t_archive is broken

CREATE OR REPLACE PROCEDURE test_ei_variable_call AS $$
DECLARE
    v_sql VARCHAR2(10000);
BEGIN
    -- Direct CALL — should create edge
    CALL other_proc();

    -- Dynamic CALL via variable — parsed_query = None -> cannot resolve
    v_sql := 'CALL other_proc()';
    EXECUTE IMMEDIATE v_sql;

    -- Parenthesized variable — same problem but with Parenthesized wrapper
    EXECUTE IMMEDIATE (v_sql);
END;
$$;
