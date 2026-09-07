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
