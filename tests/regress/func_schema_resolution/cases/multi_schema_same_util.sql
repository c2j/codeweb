-- Bug #1+#2 systemic reproduction: three schemas share the same utility
-- function name, mimicking a real enterprise database where common helper
-- functions (format_date, get_status, check_perm, etc.) are duplicated
-- across schemas.
--
-- Every unqualified call to these functions becomes Unresolved because
-- bare_name_lower has 3 candidates. This demonstrates why "较多原本的func类型"
-- (many func-type nodes) end up unresolved in practice.
--
-- Expected after fix: batch_run → format_date edge exists, zero Unresolved.
-- Pre-fix: one Unresolved node (raw_expr="format_date") is created.

CREATE SCHEMA mod_a;
CREATE SCHEMA mod_b;
CREATE SCHEMA mod_c;

CREATE FUNCTION mod_a.format_date(d DATE) RETURNS VARCHAR AS $$
BEGIN
    RETURN d::text;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION mod_b.format_date(d DATE) RETURNS VARCHAR AS $$
BEGIN
    RETURN to_char(d, 'YYYY-MM-DD');
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION mod_c.format_date(d DATE) RETURNS VARCHAR AS $$
BEGIN
    RETURN to_char(d, 'DD/MM/YYYY');
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE batch_run AS $$
DECLARE
    v_str VARCHAR;
BEGIN
    v_str := format_date(SYSDATE);
END;
$$;
