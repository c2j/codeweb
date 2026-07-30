-- Issue #58: Multi-caller shared unresolved node produces incorrect cross-schema edges.
-- Two schemas each define compute(); two procedures in different schemas both call
-- bare compute(42). The fix uses per-edge resolution when Strategy 5 (CallerSchema)
-- is the deciding factor with multiple distinct caller schemas.
--
-- Expected:
--   s1.proc_a → s1.compute  (caller schema match)
--   s2.proc_b → s2.compute  (caller schema match)

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
