-- Bug #3 reproduction: nested routine's local variables leak into the
-- enclosing procedure's scope.
--
-- A nested procedure declares v_shadow locally. After the nested block
-- returns, v_shadow stays in local_vars. The outer body's real call to
-- the v_shadow function is then wrongly filtered.
--
-- Expected after fix: DirectCall edge  outer_proc -> v_shadow EXISTS.
-- Pre-fix: the edge is MISSING (suppressed by the leak).

CREATE FUNCTION v_shadow(p INT) RETURNS INT AS $$
BEGIN
    RETURN p;
END;
$$;

CREATE PROCEDURE outer_proc AS $$
DECLARE
    PROCEDURE nested IS
        TYPE t_coll IS TABLE OF INT;
        v_shadow t_coll;
        v_idx INT := 1;
    BEGIN
        IF v_shadow(v_idx) = 1 THEN
            NULL;
        END IF;
    END;
    v_result INT;
BEGIN
    v_result := v_shadow(1);
END;
$$;
