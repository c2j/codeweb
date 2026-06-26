-- Bug reproduction: package name casing mismatch between head and body.
--
-- Head declares package "my_pkg" (lowercase), body implements "MY_PKG" (uppercase).
-- Current bug: GraphBuilder.create_package_nodes doesn't lowercase the qualified key,
-- so my_pkg and MY_PKG become two separate entries in package_index HashMap,
-- producing TWO Package nodes instead of one merged node.
--
-- Expected after fix: exactly 1 Package node, exactly 1 Procedure node (not partial).

CREATE OR REPLACE PACKAGE my_pkg AS
    PROCEDURE proc_a(p_id INT);
END my_pkg;
/

CREATE OR REPLACE PACKAGE BODY MY_PKG AS
    PROCEDURE proc_a(p_id INT) IS
    BEGIN
        NULL;
    END;
END MY_PKG;
/
