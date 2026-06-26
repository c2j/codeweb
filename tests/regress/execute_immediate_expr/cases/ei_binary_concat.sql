-- Test: EXECUTE IMMEDIATE with BinaryOp concatenation (BinaryOp escape test)
--
-- Pattern: string concatenation building dynamic SQL.
-- ogsql-parser parses `'UPDATE ' || tab_name || ' SET x=1'` as nested BinaryOp.
-- The Debug format is `BinaryOp { left: BinaryOp { left: Literal("UPDATE "), ... }, op: "||", ... }`
-- noise_rule catches `BinaryOp ` prefix -> no Unresolved node (this case WORKS).
--
-- Expected: No Unresolved node (caught by noise_rule).
-- However, the sql_text stored in body_sql will be the Debug string "BinaryOp { ... }",
-- which pollutes SQL search results. This test detects that secondary problem.

CREATE OR REPLACE PROCEDURE test_ei_binary_concat AS $$
DECLARE
    tab_name VARCHAR2(100) := 't_orders';
BEGIN
    EXECUTE IMMEDIATE 'UPDATE ' || tab_name || ' SET status = ''DONE''';
END;
$$;
