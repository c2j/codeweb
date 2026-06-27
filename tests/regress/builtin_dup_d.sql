-- File D: another procedure body calls ascii + another builtin substr
CREATE OR REPLACE PROCEDURE proc_dup_test2 AS
    v1 INT;
    v2 VARCHAR2(100);
BEGIN
    v1 := ascii('w');
    v2 := substr('hello', 1, 3);
END;
/
