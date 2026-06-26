-- Regression: a package-level user-defined collection TYPE used as a procedure
-- PARAMETER datatype (`i_a1 IN vchartab_pkg1`), combined with a local variable
-- initialised via the TYPE constructor (`vchar vchartab_pkg1 := vchartab_pkg1()`)
-- and subscripted (`vchar(1) := 500`), must NOT produce any false call edge or
-- Unresolved node.
--
-- Angle covered beyond existing cases: the package TYPE name appears in a
-- parameter's datatype position. The parameter `i_a1`, the local collection
-- variable `vchar`, and the constructor `vchartab_pkg1()` must all be filtered
-- out so none of the parenthesised usages become call edges.
--
-- Expected: zero Unresolved nodes, no call edges.

CREATE OR REPLACE PACKAGE BODY PKG_CLR_RULE_OPT1 AS
    TYPE vchartab_pkg1 IS TABLE OF NUMBER;

    PROCEDURE use_pkg_var1(i_a1 IN vchartab_pkg1) IS
        vchar vchartab_pkg1 := vchartab_pkg1();
    BEGIN
        vchar(1) := 500;
    END use_pkg_var1;
END PKG_CLR_RULE_OPT1;
