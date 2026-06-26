-- Bug #1+#2+#6 reproduction: standalone function + package member share
-- the same bare name; unqualified call from outside the package.
--
-- biz.helper (standalone, schema=Some("biz"), package=None) and
-- util_pkg.helper (package member, package=Some("util_pkg")) both
-- contribute to bare_name_lower["helper"], making it ambiguous.
--
-- Additionally, the schema-as-package index (Bug #6) only indexes standalone
-- routines whose package is None, so pkg_member_lower[("biz", "helper")]
-- exists but pkg_member_lower[("util_pkg", "helper")] also exists — Strategy 3
-- cannot help because the call is unqualified (no dot in raw_expr).
--
-- Expected after fix: do_work → helper edge exists, zero Unresolved.
-- Pre-fix: one Unresolved node (raw_expr="helper") is created.

CREATE SCHEMA biz;

CREATE FUNCTION biz.helper(x INT) RETURNS INT AS $$
BEGIN
    RETURN x;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PACKAGE util_pkg IS
    FUNCTION helper(x INT) RETURN INT;
END util_pkg;

CREATE OR REPLACE PACKAGE BODY util_pkg IS
    FUNCTION helper(x INT) RETURN INT IS
    BEGIN
        RETURN x * 2;
    END;
END util_pkg;

CREATE PROCEDURE do_work AS $$
DECLARE
    v INT;
BEGIN
    v := helper(42);
END;
$$;
