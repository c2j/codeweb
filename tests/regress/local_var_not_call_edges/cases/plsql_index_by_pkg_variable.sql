-- Regression: package-level Variable and TYPE declarations must be visible
-- to every routine in the package, so collection indexing `vchar_array_pkg(1)`
-- on a package-level associative array (INDEX BY table) is NOT captured as
-- a procedure call edge.
--
-- Bug: collect_package_call_edges only walked Procedure/Function bodies and
-- skipped PackageItem::Variable / PackageItem::Type. The package variable
-- `vchar_array_pkg` was never registered in local_vars, so
-- ogsql-parser's PlProcedureCall interpretation of `vchar_array_pkg(1) := 'x'`
-- produced a false `unresolved:vchar_array_pkg` node (visit_procedure_call
-- also lacked the local_vars/known_types filter, now applied).
--
-- Expected: zero Unresolved nodes (and no false call edges).

CREATE OR REPLACE PACKAGE BODY PKG_CLR_RULE_OPT AS
    TYPE vchartab_pkg IS TABLE OF VARCHAR2(4000) INDEX BY INTEGER;
    vchar_array_pkg   vchartab_pkg;

    PROCEDURE use_pkg_var IS
    BEGIN
        vchar_array_pkg(1) := 'x';
    END use_pkg_var;
END PKG_CLR_RULE_OPT;
