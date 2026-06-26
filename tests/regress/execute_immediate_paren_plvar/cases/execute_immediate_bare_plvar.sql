-- Control case for execute_immediate_paren_plvar.sql.
--
-- Same scenario but WITHOUT the extra parentheses around the PL variable:
--     EXECUTE IMMEDIATE v_sql3;
--
-- ogsql-parser parses this as Expr::PlVariable(["v_sql3"]) (no Parenthesized
-- wrapper), so the Debug string is `PlVariable(["v_sql3"])` which IS caught by
-- noise_rule's `starts_with("PlVariable(")` check.
--
-- This case should already pass (0 Unresolved nodes) and serves as a contrast
-- to prove the bug is specifically about the Parenthesized wrapper, not about
-- PL variables in EXECUTE IMMEDIATE in general.

CREATE OR REPLACE PROCEDURE run_dynamic_bare AS $$
DECLARE
    v_sql3 VARCHAR2(10000) := '';
BEGIN
    v_sql3 := 'UPDATE t SET x = 1';
    EXECUTE IMMEDIATE v_sql3;
END;
$$;
