-- File C: procedure body calls ascii
CREATE OR REPLACE PROCEDURE proc_dup_test AS
BEGIN
    SELECT ascii('z') INTO v FROM dual;
END;
/
