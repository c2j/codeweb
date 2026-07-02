-- Case 02: Procedure - Direct CALL + Indirect caller-chain (3-level transitive closure)
-- Target: proc_c
-- Graph: proc_a → proc_b → proc_c  (DirectCall edges)

CREATE OR REPLACE PROCEDURE proc_c()
AS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE proc_b()
AS
BEGIN
    CALL proc_c();
END;
/

CREATE OR REPLACE PROCEDURE proc_a()
AS
BEGIN
    CALL proc_b();
END;
/

-- Unrelated procedure (should NOT match)
CREATE OR REPLACE PROCEDURE unrelated_proc()
AS
BEGIN
    NULL;
END;
/
