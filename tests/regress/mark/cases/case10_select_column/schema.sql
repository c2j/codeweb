-- Case 10: SELECT with function-like column/table names must NOT falsely match
-- Target: function `calc_total`
--     caller: function `calc_subtotal` calls calc_total
-- Expected: SELECT columns named like functions should NOT match

CREATE OR REPLACE FUNCTION calc_total(amount NUMERIC) RETURNS NUMERIC
AS
BEGIN
    RETURN amount * 1.1;
END;
/

CREATE OR REPLACE FUNCTION calc_subtotal(base NUMERIC) RETURNS NUMERIC
AS
    v_result NUMERIC;
BEGIN
    v_result := calc_total(base);
    RETURN v_result;
END;
/
