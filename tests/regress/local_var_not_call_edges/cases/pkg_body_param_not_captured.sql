-- Bug #2 reproduction: package body procedure parameters are never registered
-- in local_vars because visit_statement doesn't fire for package items.
--
-- p_ids is a parameter of process_data. Used with (index) syntax in WHERE,
-- it should be filtered — but it isn't, producing a false Unresolved node.
--
-- Expected after fix: zero Unresolved nodes.
-- Pre-fix: one Unresolved node (raw_expr="p_ids") is created.

CREATE TABLE t_audit (id INT);

CREATE OR REPLACE PACKAGE BODY param_pkg IS
    PROCEDURE process_data(p_ids VARCHAR) IS
        v_idx INT := 1;
    BEGIN
        DELETE FROM t_audit WHERE id = p_ids(v_idx);
    END process_data;
END param_pkg;
