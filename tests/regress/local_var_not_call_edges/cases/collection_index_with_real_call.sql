-- Regression: collection index access must NOT mask real function calls.
--
-- Verifies the fix doesn't over-filter — real calls still get captured while
-- local collection accesses are skipped in the same procedure body.
--
-- Placement matters for triggering the bug: the collection access must appear
-- in a SQL expression position (here the DELETE WHERE clause, walked via
-- walk_delete → walk_expr) so visit_expr fires on it. PL/pgSQL assignment
-- *targets* (`v := ...`) are not walked, only the RHS.
--
-- Expected: exactly one DirectCall edge  clean_proc -> compute_score.
--           NO DirectCall edge            clean_proc -> v_scores.

CREATE FUNCTION compute_score(p INT) RETURNS INT AS $$
BEGIN
    RETURN p * 10;
END;
$$;

CREATE TABLE t_log (id INT, score INT);

CREATE PROCEDURE clean_proc AS $$
DECLARE
    TYPE t_scores IS TABLE OF INT;
    v_scores t_scores;
    v_idx INTEGER := 1;
    v_result INT;
BEGIN
    v_result := compute_score(5);                  -- real call (RHS): must be captured
    DELETE FROM t_log WHERE id = v_scores(v_idx);  -- collection access in WHERE: must NOT
END;
$$;
