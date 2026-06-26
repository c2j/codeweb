-- Test: EXECUTE IMMEDIATE with literal CALL (working case)
--
-- Pattern: `EXECUTE IMMEDIATE 'CALL other_proc()'`
-- ogsql-parser CAN parse the inner SQL string and set parsed_query = Some(CallFuncStatement).
-- When parsed_query is Some, the Visitor walks into it via the default walk_pl_statement,
-- triggering visit_call() which creates a DirectCall edge. No Unresolved node created.
--
-- Also tests: direct CALL other_proc() (baseline working case).
--
-- Expected:
--   - 0 Unresolved nodes
--   - DirectCall edge from test_ei_literal_call -> other_proc (from both the direct CALL
--     and the EXECUTE IMMEDIATE 'CALL ...' if parsed_query is Some)
--   - TableAccess edge from other_proc -> t_archive

CREATE OR REPLACE PROCEDURE other_proc AS $$
BEGIN
    UPDATE t_archive SET status = 'done';
END;
$$;

CREATE OR REPLACE PROCEDURE test_ei_literal_call AS $$
BEGIN
    -- Baseline: direct CALL (always works)
    CALL other_proc();

    -- Dynamic CALL via literal string (parsed_query = Some(...)) -> should resolve
    EXECUTE IMMEDIATE 'CALL other_proc()';
END;
$$;
