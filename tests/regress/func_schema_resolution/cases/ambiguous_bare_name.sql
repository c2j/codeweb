-- Bug #1+#2 reproduction: ambiguous bare name across schemas.
--
-- Two schemas define a function with the same bare name. A caller invokes
-- the function without schema qualification. The post-processing pass
-- (resolve_unresolved_nodes → try_resolve_routine Strategy 4) finds TWO
-- candidates in bare_name_lower and gives up because matches.len() != 1,
-- leaving an Unresolved node.
--
-- Root cause: create_edges has no "bare-name → schema-qualified definition"
-- fallback; resolve_unresolved_nodes Strategy 4 requires unique match.
--
-- Expected after fix: caller_proc → util_func edge exists, zero Unresolved.
-- Pre-fix: one Unresolved node (raw_expr="util_func") is created.

CREATE SCHEMA app_a;
CREATE SCHEMA app_b;

CREATE FUNCTION app_a.util_func(x INT) RETURNS INT AS $$
BEGIN
    RETURN x;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION app_b.util_func(x INT) RETURNS INT AS $$
BEGIN
    RETURN x * 2;
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE caller_proc AS $$
DECLARE
    v INT;
BEGIN
    v := util_func(42);
END;
$$;
