-- Bug reproduction: standalone procedure name casing mismatch.
--
-- Two CREATE PROCEDURE statements for what is semantically the same procedure,
-- using different casing. In openGauss/PostgreSQL, unquoted identifiers fold to
-- lowercase, so MY_TEST_PROC and my_test_proc are the same object.
-- Current bug: builder.rs `proc_index` uses raw-cased RoutineId keys,
-- so my_test_proc and MY_TEST_PROC produce TWO Procedure nodes.
--
-- Expected after fix: exactly 1 Procedure node (merged case-insensitively).
-- A caller_proc is included so the merged node has an incident edge (non-orphan).

CREATE OR REPLACE PROCEDURE my_test_proc IS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE MY_TEST_PROC IS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE caller_proc AS
    v_dummy INT;
BEGIN
    v_dummy := 0;
    my_test_proc();
END;
/
