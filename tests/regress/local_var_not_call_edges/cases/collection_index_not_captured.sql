-- Regression: PL/SQL collection variable index access must NOT be captured
-- as a procedure call edge.
--
-- Bug: CallExtractor.visit_expr treats every Expr::FunctionCall { builtin: None }
-- as a call edge. Oracle-style collection indexing uses paren syntax: v_date(i).
-- Without a local symbol table built from DECLARE, v_date / v_fund are pushed
-- as call targets and end up as Unresolved nodes.
--
-- Mirrors the real-world report: TYPE t_date IS TABLE OF VARCHAR(8); v_date t_date;
-- then `WHERE work_date = v_date(v_clt_num)` creates a false edge to `v_date`.
--
-- Expected: zero DirectCall/DynamicCall edges — there is no real procedure call
-- in this file, only collection variable reads.

CREATE TABLE jk_rcs (data_date VARCHAR(8));
CREATE TABLE deal_gather (fund_code VARCHAR(9), work_date VARCHAR(8));

CREATE PROCEDURE clean_deal_gather AS $$
DECLARE
    TYPE t_fund IS TABLE OF VARCHAR(9);
    TYPE t_date IS TABLE OF VARCHAR(8);
    v_fund t_fund;
    v_date t_date;
    v_clt_num INTEGER := 0;
BEGIN
    SELECT t.data_date BULK COLLECT INTO v_date FROM jk_rcs t;

    FOR v_clt_num IN 1 .. v_fund.COUNT LOOP
        DELETE FROM deal_gather
        WHERE fund_code = v_fund(v_clt_num)
          AND work_date = v_date(v_clt_num);
    END LOOP;
END;
$$;
