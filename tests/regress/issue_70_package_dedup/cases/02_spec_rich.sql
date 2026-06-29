-- Issue #70: Package spec — richer spec (should win per CREATE OR REPLACE last-wins).
-- This file declares pkg_x with var_a, var_b, var_c. codeweb's first-wins
-- dedup may keep the stale spec from 01_spec_stale.sql instead.

CREATE OR REPLACE PACKAGE pkg_x IS
    var_a INT;
    var_b VARCHAR2(100);
    var_c NUMBER;
END pkg_x;
/
