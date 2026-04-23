-- ============================================================
-- Package: pkg_audit — audit logging
-- ============================================================

CREATE OR REPLACE FUNCTION pkg_audit.log_transfer(
    p_user_id BIGINT,
    p_org_id  BIGINT
) RETURNS VOID AS $$
BEGIN
    INSERT INTO t_audit_log(action, ref_id, detail)
    VALUES('TRANSFER', p_user_id, 'org=' || p_org_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_audit.purge_old_logs(
    p_days INTEGER
) AS $$
BEGIN
    DELETE FROM t_audit_log WHERE created_at < now() - INTERVAL '1 day' * p_days;
END;
$$ LANGUAGE plpgsql;
