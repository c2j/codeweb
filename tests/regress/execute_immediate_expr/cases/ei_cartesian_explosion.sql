-- Regression: v0.7.10 introduced unbounded cartesian-product expansion in
-- extract_all_literal_strings for `||` chains whose operands carry multiple
-- values (CASE/IF branches). 20 binary CASE terms → 2^20 ≈ 1M literal-string
-- variants → CPU spin + OOM kill on a single procedure.
--
-- The MAX_VALUE_SET cap (extractor.rs) must abort static expansion past 64
-- variants, degrading EXECUTE IMMEDIATE resolution to an opaque dynamic call.
-- This file must parse + extract in well under a second, not hang.
CREATE OR REPLACE PROCEDURE ei_cartesian_explosion AS
BEGIN
    DECLARE
        v_sql VARCHAR2(4000);
    BEGIN
        v_sql :=
            'CALL ' ||
            CASE WHEN 1=1 THEN 'p1' ELSE 'p2' END ||
            CASE WHEN 1=1 THEN 'a1' ELSE 'b1' END ||
            CASE WHEN 1=1 THEN 'a2' ELSE 'b2' END ||
            CASE WHEN 1=1 THEN 'a3' ELSE 'b3' END ||
            CASE WHEN 1=1 THEN 'a4' ELSE 'b4' END ||
            CASE WHEN 1=1 THEN 'a5' ELSE 'b5' END ||
            CASE WHEN 1=1 THEN 'a6' ELSE 'b6' END ||
            CASE WHEN 1=1 THEN 'a7' ELSE 'b7' END ||
            CASE WHEN 1=1 THEN 'a8' ELSE 'b8' END ||
            CASE WHEN 1=1 THEN 'a9' ELSE 'b9' END ||
            CASE WHEN 1=1 THEN 'aa' ELSE 'ba' END ||
            CASE WHEN 1=1 THEN 'ab' ELSE 'bb' END ||
            CASE WHEN 1=1 THEN 'ac' ELSE 'bc' END ||
            CASE WHEN 1=1 THEN 'ad' ELSE 'bd' END ||
            CASE WHEN 1=1 THEN 'ae' ELSE 'be' END ||
            CASE WHEN 1=1 THEN 'af' ELSE 'bf' END ||
            CASE WHEN 1=1 THEN 'ag' ELSE 'bg' END ||
            CASE WHEN 1=1 THEN 'ah' ELSE 'bh' END ||
            CASE WHEN 1=1 THEN 'ai' ELSE 'bi' END ||
            CASE WHEN 1=1 THEN 'aj' ELSE 'bj' END;
        EXECUTE IMMEDIATE v_sql;
    END;
END;
/
