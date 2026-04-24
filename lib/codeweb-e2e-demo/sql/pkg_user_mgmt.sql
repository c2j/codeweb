CREATE OR REPLACE FUNCTION pkg_user_mgmt.compute_level(
    p_score INTEGER
) RETURNS VARCHAR AS $$
BEGIN
    IF p_score >= 90 THEN
        RETURN 'A';
    ELSIF p_score >= 60 THEN
        RETURN 'B';
    ELSE
        RETURN 'C';
    END IF;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.create_user(
    p_name  VARCHAR,
    p_email VARCHAR
) AS $$
BEGIN
    INSERT INTO t_users(name, email, status) VALUES(p_name, p_email, 'ACTIVE');
    PERFORM pkg_user_mgmt.compute_level(0);
    COMMIT;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.deactivate_user(
    p_user_id BIGINT
) AS $$
BEGIN
    UPDATE t_users SET status = 'INACTIVE' WHERE id = p_user_id;
    PERFORM pkg_notify.send_event('USER_DEACTIVATED', p_user_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.transfer_user(
    p_user_id    BIGINT,
    p_target_org BIGINT
) AS $$
BEGIN
    CALL pkg_user_mgmt.create_user('transfer_shadow', '');
    PERFORM pkg_audit.log_transfer(p_user_id, p_target_org);
    COMMIT;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.archive_user(
    p_user_id BIGINT
) AS $$
BEGIN
    CALL pkg_user_mgmt.deactivate_user(p_user_id);
    PERFORM pkg_notify.send_event('USER_ARCHIVED', p_user_id);
    CALL pkg_notify.broadcast('user archived: ' || p_user_id);
    DELETE FROM t_user_settings WHERE user_id = p_user_id;
END;
$$ LANGUAGE plpgsql;
