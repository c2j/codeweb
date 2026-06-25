-- Regression: a procedure parameter must NOT be captured as a call edge when
-- used with paren syntax in a SQL expression position.
--
-- Bug class: same as collection_index_not_captured, but the collection-typed
-- identifier arrives via the parameter list rather than DECLARE. The fix in
-- CallExtractor.visit_statement must collect parameter names into local_vars
-- alongside the DECLARE-section variables.
--
-- Expected: DirectCall edge  batch_check -> real_target.
--           NO Unresolved node (p_ids must not spawn one).

CREATE FUNCTION real_target(p INT) RETURNS INT AS $$
BEGIN
    RETURN p + 1;
END;
$$;

CREATE TABLE t_audit (id INT);

CREATE PROCEDURE batch_check(p_ids VARCHAR) AS $$
DECLARE
    v_idx INTEGER := 1;
    v_result INT;
BEGIN
    v_result := real_target(5);                   -- real call (RHS): captured
    DELETE FROM t_audit WHERE id = p_ids(v_idx);  -- param access in WHERE: NOT captured
END;
$$;
