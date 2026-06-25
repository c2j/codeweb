-- Bug #3 reproduction: nested routine's local variables leak into the
-- enclosing procedure's scope.
--
-- A nested procedure (declared inside a package body item's DECLARE block)
-- declares v_shadow locally. After the nested block returns, v_shadow must
-- be removed from local_vars so the outer body's real call to the v_shadow
-- function is captured.
--
-- Uses PACKAGE BODY because ogsql-parser only parses nested procedures
-- inside package items, not inside standalone $$ blocks.
--
-- Expected: DirectCall edge  outer_proc -> v_shadow EXISTS.

CREATE FUNCTION v_shadow(p INT) RETURNS INT AS $$
BEGIN
    RETURN p;
END;
$$;

CREATE OR REPLACE PACKAGE BODY nest_pkg IS
    PROCEDURE outer_proc IS
        PROCEDURE nested IS
            v_shadow INT;
        BEGIN
            NULL;
        END nested;
        v_result INT;
    BEGIN
        v_result := v_shadow(1);
    END outer_proc;
END nest_pkg;
