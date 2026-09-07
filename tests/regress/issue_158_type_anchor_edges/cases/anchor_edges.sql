-- Issue #158: %TYPE/%ROWTYPE anchor edges must coexist with (not replace)
-- normal TableAccess DML edges, and a table referenced only through a
-- %TYPE variable (never in DML) must still get an inferred AnchorsOn-only
-- table node.
--
-- Simplified, parseable equivalent of the real-world issue #158 sample:
-- the RETURN clause, a `RESULT` variable, and a `v_purchase_days` variable
-- all anchor to the same DML-read column (par_sys_purchase.purchase_days),
-- while `v_repurchase_date` anchors to a table (dat_trd_repurchase) that
-- is never referenced in DML.
CREATE OR REPLACE FUNCTION BIGFUND.FNC_GET_PURCHASE_JS_DAYS
RETURN par_sys_purchase.purchase_days%TYPE
IS
    RESULT par_sys_purchase.purchase_days%TYPE;
    v_purchase_days par_sys_purchase.purchase_days%TYPE;
    v_repurchase_date dat_trd_repurchase.purchase_date%TYPE;
BEGIN
    SELECT t.purchase_days INTO v_purchase_days FROM par_sys_purchase t;
    RESULT := v_purchase_days;
    RETURN RESULT;
END;
