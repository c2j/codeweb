-- Issue #58: Multi-caller shared unresolved node produces incorrect cross-schema edges.
-- Two schemas each define compute(); two procedures in different schemas both call
-- bare compute(42). The shared unresolved node is resolved to ONE target based on
-- the first caller's schema (s1), so s2.proc_b gets an incorrect edge to s1.compute.
--
-- Expected after fix:
--   s1.proc_a → s1.compute  (caller schema match)
--   s2.proc_b → s2.compute  (caller schema match)
-- Currently (bug):
--   s1.proc_a → s1.compute  ✓ (first caller wins by accident)
--   s2.proc_b → s1.compute  ✗ (should be s2.compute)

CREATE SCHEMA s1;
CREATE SCHEMA s2;

CREATE FUNCTION s1.compute(x INT) RETURNS INT AS
BEGIN
    RETURN x;
END;
/

CREATE FUNCTION s2.compute(x INT) RETURNS INT AS
BEGIN
    RETURN x * 2;
END;
/

CREATE PROCEDURE s1.proc_a AS
DECLARE v INT;
BEGIN
    v := compute(42);
END;
/

CREATE PROCEDURE s2.proc_b AS
DECLARE v INT;
BEGIN
    v := compute(42);
END;
/
