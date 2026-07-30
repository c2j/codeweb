-- Issue #70: Package spec — richer spec (should win per CREATE OR REPLACE last-wins).
-- Declares var_b/var_c AND the TYPE vchar_array which the body (03_body.sql)
-- uses. When this spec wins, the body inherits vchar_array as a registered type
-- so its constructor call vchar_array() is NOT misread as a procedure call.

CREATE OR REPLACE PACKAGE pkg_x IS
    var_a INT;
    var_b VARCHAR2(100);
    var_c NUMBER;
    TYPE vchar_array IS TABLE OF VARCHAR2(100);
END pkg_x;
/
