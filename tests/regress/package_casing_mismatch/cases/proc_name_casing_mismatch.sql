-- Bug reproduction: procedure name casing mismatch between head declaration and body implementation.
--
-- Head declares PROCEDURE proc_a (lowercase), body implements PROCEDURE Proc_A (mixed case).
-- Current bug: the spec/body reconciliation loop compares pn == pkg_name and rn == routine_name
-- using case-sensitive == on String, so "proc_a" != "Proc_A". The head-declared routine becomes
-- a partial:true orphan node, while the body creates a separate full node.
--
-- Expected after fix: exactly 1 Procedure node (merged, not partial).

CREATE OR REPLACE PACKAGE my_pkg AS
    PROCEDURE proc_a(p_id INT);
END my_pkg;
/

CREATE OR REPLACE PACKAGE BODY my_pkg AS
    PROCEDURE Proc_A(p_id INT) IS
    BEGIN
        NULL;
    END;
END my_pkg;
/
