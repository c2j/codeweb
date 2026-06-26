-- Comprehensive test: ALL EXECUTE IMMEDIATE patterns in one file
--
-- Purpose: Analyze the complete impact of dynamic SQL on graph accuracy.
-- This file contains multiple procedures that use different EXECUTE IMMEDIATE
-- forms, plus a shared callee procedure and a table.
--
-- Graph analysis questions:
--   1. Which patterns produce Unresolved nodes?
--   2. Which patterns create DirectCall vs DynamicCall edges?
--   3. Are CALL targets inside literal EXECUTE IMMEDIATE resolved?
--   4. Are TableAccess edges lost for dynamic SQL paths?
--   5. Does body_sql contain Debug strings (search pollution)?

-- ── Shared callee ──
CREATE OR REPLACE PROCEDURE shared_callee AS $$
BEGIN
    INSERT INTO t_result VALUES ('called');
END;
$$;

-- ── Pattern 1: Direct CALL (baseline — always works) ──
CREATE OR REPLACE PROCEDURE p_direct_call AS $$
BEGIN
    CALL shared_callee();
END;
$$;

-- ── Pattern 2: EXECUTE IMMEDIATE with literal CALL ──
-- parsed_query = Some(CallFuncStatement) -> walks into CALL -> resolved
CREATE OR REPLACE PROCEDURE p_ei_literal_call AS $$
BEGIN
    EXECUTE IMMEDIATE 'CALL shared_callee()';
END;
$$;

-- ── Pattern 3: EXECUTE IMMEDIATE with literal SELECT ──
-- parsed_query = Some(SelectStatement) -> walks into SELECT -> table access resolved
CREATE OR REPLACE PROCEDURE p_ei_literal_select AS $$
BEGIN
    EXECUTE IMMEDIATE 'SELECT * FROM t_source WHERE id = 1';
END;
$$;

-- ── Pattern 4: EXECUTE IMMEDIATE with literal DML ──
-- parsed_query = Some(UpdateStatement) -> walk_select? Actually visit_update -> table access resolved
CREATE OR REPLACE PROCEDURE p_ei_literal_update AS $$
BEGIN
    EXECUTE IMMEDIATE 'UPDATE t_target SET x = 2 WHERE id = 1';
END;
$$;

-- ── Pattern 5: EXECUTE IMMEDIATE with bare variable (PlVariable) ──
-- parsed_query = None -> DynamicCall via format!("{:?}", Peeled(string_expr))
-- noise_rule catches "PlVariable(" -> removed as noise (currently works only for bare)
CREATE OR REPLACE PROCEDURE p_ei_bare_var AS $$
DECLARE
    v_sql VARCHAR2(10000);
BEGIN
    v_sql := 'CALL shared_callee()';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 6: EXECUTE IMMEDIATE with parenthesized variable ──
-- parsed_query = None -> peel_parenthesized then format!("{:?}", ...)
-- After peeling: PlVariable -> noise_rule catches "PlVariable("
CREATE OR REPLACE PROCEDURE p_ei_paren_var AS $$
DECLARE
    v_sql VARCHAR2(10000);
BEGIN
    v_sql := 'CALL shared_callee()';
    EXECUTE IMMEDIATE (v_sql);
END;
$$;

-- ── Pattern 7: EXECUTE IMMEDIATE with record field access ──
-- parsed_query = None -> format!("{:?}", FieldAccess{...}) -> escapes noise_rule
CREATE OR REPLACE PROCEDURE p_ei_field_access AS $$
DECLARE
    r RECORD;
BEGIN
    r.sql_text := 'CALL shared_callee()';
    EXECUTE IMMEDIATE r.sql_text;
END;
$$;

-- ── Pattern 8: EXECUTE IMMEDIATE with string concatenation ──
-- parsed_query = None -> format!("{:?}", BinaryOp{...}) -> noise_rule catches "BinaryOp "
CREATE OR REPLACE PROCEDURE p_ei_concat AS $$
DECLARE
    tbl VARCHAR2(100) := 't_target';
BEGIN
    EXECUTE IMMEDIATE 'UPDATE ' || tbl || ' SET x = 3';
END;
$$;

-- ── Pattern 9: EXECUTE IMMEDIATE with multiple CTX variable forms ──
-- Test what kind of Expr wrapper surrounds the variable
CREATE OR REPLACE PROCEDURE p_ei_complex_chain AS $$
DECLARE
    v_sql VARCHAR2(10000);
    v_tbl  VARCHAR2(100) := 't_target';
BEGIN
    -- Direct CALL (creates edge)
    CALL shared_callee();

    -- Dynamic via variable (parsed_query = None)
    v_sql := 'INSERT INTO t_result VALUES (1)';
    EXECUTE IMMEDIATE v_sql;

    -- Dynamic via concatenation
    EXECUTE IMMEDIATE 'INSERT INTO ' || v_tbl || ' VALUES (2)';
END;
$$;
