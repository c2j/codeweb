-- Issue #70: Package BODY for pkg_x.
-- do_work references the TYPE vchar_array (declared ONLY in 02_spec_rich.sql).
-- Bug (first-wins, stale spec 01): vchar_array unknown to the body's extraction
-- scope → vchar_array() misread as a call → spurious Unresolved node
-- {raw_expr: "vchar_array"} + spurious do_work --direct--> edge.
-- Fix (last-wins, rich spec 02): vchar_array registered as a type → no edge.

CREATE OR REPLACE PACKAGE BODY pkg_x IS
    PROCEDURE do_work IS
        l_data vchar_array;
    BEGIN
        l_data := vchar_array();
        l_data(1) := var_b;
    END do_work;
END pkg_x;
/
