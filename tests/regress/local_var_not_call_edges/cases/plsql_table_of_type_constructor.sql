-- Regression: PL/SQL block-local TYPE ... IS TABLE OF ... declarations
-- (including `%ROWTYPE` element types and INDEX BY variants) must be
-- recognised as type names so the empty-constructor call `t_work_array()`
-- is NOT captured as a function call edge.
--
-- Bug: same root cause as plsql_varray_type_constructor — PlDeclaration::Type
-- was ignored by CallExtractor. The empty constructor `t_work_array()`
-- produced a false `unresolved:t_work_array` node.
--
-- Expected: zero Unresolved nodes.

CREATE OR REPLACE PROCEDURE PROC_TABLEOF AS $$
DECLARE
    CURSOR c_work IS
        SELECT wrk.serial_no
          FROM wrk
         WHERE wrk.fund_code = p_i_fundcode
           AND wrk.trade_date = p_i_tradedate
         ORDER BY wrk.serial_no;
    r_work c_work%ROWTYPE;
    TYPE t_work_array IS TABLE OF c_work%ROWTYPE;
    v_work_array t_work_array := t_work_array();
BEGIN
    NULL;
END;
$$;
