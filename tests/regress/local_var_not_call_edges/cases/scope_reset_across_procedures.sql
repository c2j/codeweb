-- Regression: local variables from one procedure must NOT leak into another.
--
-- Without local_vars.clear() on CreateProcedure entry, v_date declared in
-- proc_a would cause CallExtractor to filter any v_date(...) usage in proc_b
-- too — masking the real call to the v_date function.
--
-- proc_a declares a local collection variable named v_date (shadowing the
-- global function). proc_b calls the global v_date function via PERFORM.
-- The fix must keep proc_a's collection access silent while preserving
-- proc_b's real call edge.
--
-- Expected: DirectCall edge  proc_b -> v_date (the function).
--           NO DirectCall edge  proc_a -> v_date (local var, not a call).

CREATE FUNCTION v_date(p INT) RETURNS VARCHAR AS $$
BEGIN
    RETURN '20240101';
END;
$$;

CREATE TABLE t1 (d VARCHAR);

CREATE PROCEDURE proc_a AS $$
DECLARE
    TYPE t_date IS TABLE OF VARCHAR(8);
    v_date t_date;
    v_idx INTEGER := 1;
BEGIN
    DELETE FROM t1 WHERE d = v_date(v_idx);   -- collection access: NOT a call
END;
$$;

CREATE PROCEDURE proc_b AS $$
BEGIN
    PERFORM v_date(1);   -- REAL call to the v_date function: MUST be captured
END;
$$;
