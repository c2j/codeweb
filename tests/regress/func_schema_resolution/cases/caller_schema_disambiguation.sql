-- Bug #2 reproduction: ambiguous bare name, but caller's schema could
-- disambiguate — the resolution logic does not use caller schema context.
--
-- Caller is s1.run_it. Two schemas (s1, s2) both define compute().
-- The unqualified call compute(42) should preferentially resolve to
-- s1.compute (same schema as caller), but neither create_edges nor
-- resolve_unresolved_nodes considers the caller's schema for bare-name
-- disambiguation.
--
-- Note: create_edges Strategy 5 (caller-context fallback) only checks the
-- caller's *package*, not the caller's *schema*. For standalone schema
-- functions, this fallback does not fire.
--
-- Expected after fix: run_it → compute edge exists, zero Unresolved.
-- Pre-fix: one Unresolved node (raw_expr="compute") is created.

CREATE SCHEMA s1;
CREATE SCHEMA s2;

CREATE FUNCTION s1.compute(x INT) RETURNS INT AS $$
BEGIN
    RETURN x;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION s2.compute(x INT) RETURNS INT AS $$
BEGIN
    RETURN x * 2;
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE s1.run_it AS $$
DECLARE
    v INT;
BEGIN
    v := compute(42);
END;
$$;
