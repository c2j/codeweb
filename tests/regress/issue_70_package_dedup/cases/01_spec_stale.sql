-- Issue #70: Package spec first-wins dedup — stale spec (fewer items).
-- This file declares pkg_x with only var_a. If first-wins, the richer
-- spec in 02_spec_rich.sql is ignored. Database CREATE OR REPLACE
-- semantics are last-wins, so 02_spec_rich.sql should be canonical.

CREATE OR REPLACE PACKAGE pkg_x IS
    var_a INT;
END pkg_x;
/
