-- Test: EXECUTE IMMEDIATE rec.field (FieldAccess escape test)
--
-- Real-world pattern: record variable with a VARCHAR2 field holding dynamic SQL.
-- ogsql-parser parses `rec.sql_text` as Expr::FieldAccess { object: PlVariable(["rec"]), field: "sql_text" }
-- The Debug format is `FieldAccess { object: PlVariable(["rec"]), field: "sql_text" }`
-- noise_rule only catches: PlVariable(, BinaryOp , BinaryOp{, FunctionCall , FunctionCall{, Literal(, ColumnRef(
-- -> FieldAccess ESCAPES the filter -> produces an Unresolved node.
--
-- Expected after fix: No Unresolved node whose raw_expr contains "FieldAccess".

CREATE OR REPLACE PROCEDURE test_ei_field_access AS $$
DECLARE
    r_cfg RECORD;
BEGIN
    r_cfg.sql_text := 'UPDATE t SET x = 1';
    EXECUTE IMMEDIATE r_cfg.sql_text;
END;
$$;
