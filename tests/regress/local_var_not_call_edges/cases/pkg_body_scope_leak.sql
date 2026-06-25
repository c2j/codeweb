-- Bug #1 reproduction: package body local variable leaks across sibling
-- procedures, suppressing real call edges.
--
-- proc_a declares a local collection variable named "helper_fn" (shadowing
-- the global function). proc_b calls the global helper_fn function. Because
-- collect_package_call_edges never clears local_vars between items, proc_a's
-- "helper_fn" leaks into proc_b's scope — the real call is filtered.
--
-- Expected after fix: DirectCall edge  proc_b -> helper_fn EXISTS.
-- Pre-fix: the edge is MISSING (suppressed by the leak).

CREATE FUNCTION helper_fn(p INT) RETURNS INT AS $$
BEGIN
    RETURN p;
END;
$$;

CREATE OR REPLACE PACKAGE BODY scope_leak_pkg IS
    PROCEDURE proc_a IS
        TYPE t_coll IS TABLE OF INT;
        helper_fn t_coll;
        v_idx INT := 1;
    BEGIN
        IF helper_fn(v_idx) = 1 THEN
            NULL;
        END IF;
    END proc_a;

    PROCEDURE proc_b IS
        v_result INT;
    BEGIN
        v_result := helper_fn(1);
    END proc_b;
END scope_leak_pkg;
