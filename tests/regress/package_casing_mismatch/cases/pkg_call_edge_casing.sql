-- Bug reproduction: package head/body casing mismatch + a caller making calls.
--
-- Head declares FUNCTION helper (lowercase), body implements FUNCTION Helper (capitalized).
-- A standalone caller proc calls helper(10) with the lowercased name.
-- Current bug: the casing mismatch produces TWO procedure nodes (partial helper in head,
-- full Helper in body). The call helper(10) resolves to the partial node or produces an
-- Unresolved node, and the call edge from caller_proc -> helper might be missing.
--
-- Expected after fix: 1 Package node, 1 Function node, DirectCall edge caller_proc -> helper.

CREATE OR REPLACE PACKAGE my_pkg AS
    FUNCTION helper(x INT) RETURN INT;
END my_pkg;
/

CREATE OR REPLACE PACKAGE BODY MY_PKG AS
    FUNCTION Helper(x INT) RETURN INT IS
    BEGIN
        RETURN x * 2;
    END;
END MY_PKG;
/

CREATE PROCEDURE caller_proc AS
    v INT;
BEGIN
    v := my_pkg.helper(10);
END;
/
