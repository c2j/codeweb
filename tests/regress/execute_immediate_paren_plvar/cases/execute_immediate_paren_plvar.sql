-- Bug repro: EXECUTE IMMEDIATE with a PARENTHESIZED PL variable produces a
-- spurious Unresolved node named `unresolved:Parenthesized(PlVariable(["v_sql3"]))`.
--
-- Real-world pattern (openGauss/GaussDB PL/SQL):
--     v_sql3 VARCHAR2(10000) := '';
--     v_sql3 := 'UPDATE ...';
--     EXECUTE IMMEDIATE (v_sql3);          -- <-- extra parens around the variable
--
-- Root cause (two coupled sites):
--   1. ogsql-parser parses `EXECUTE IMMEDIATE (v_sql3)` as a PlExecuteStmt whose
--      string_expr = Expr::Parenthesized(Expr::PlVariable(["v_sql3"])).
--   2. CallExtractor::visit_pl_statement (src/parser/extractor.rs:340) formats the
--      non-literal expr via `format!("{:?}", string_expr)`, yielding the Debug
--      string `Parenthesized(PlVariable(["v_sql3"]))`, which becomes a dynamic
--      CallEdge callee_name.
--   3. noise_rule (src/graph/builder.rs:2795) filters AST debug strings but only
--      matches `starts_with("PlVariable(")` — the `Parenthesized(` wrapper evades
--      the filter, so the node survives as Unresolved.
--
-- Expected after fix: NO Unresolved node whose name contains "Parenthesized"
-- survives; the parenthesized PL variable is noise, not a routine reference.

CREATE OR REPLACE PROCEDURE run_dynamic_paren AS $$
DECLARE
    v_sql3 VARCHAR2(10000) := '';
BEGIN
    v_sql3 := 'UPDATE t SET x = 1';
    EXECUTE IMMEDIATE (v_sql3);
END;
$$;
