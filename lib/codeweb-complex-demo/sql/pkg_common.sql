CREATE OR REPLACE FUNCTION pkg_common.get_sys_date()
RETURNS TIMESTAMP AS $$
BEGIN
    RETURN CURRENT_TIMESTAMP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION pkg_common.format_amount(
    p_amount NUMERIC
) RETURNS VARCHAR AS $$
BEGIN
    RETURN TO_CHAR(p_amount, 'FM999,999,999.00');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_common.log_operation(
    p_module VARCHAR,
    p_action VARCHAR,
    p_target_id BIGINT
) AS $$
BEGIN
    INSERT INTO t_operation_log(module, action, target_id, created_at)
    VALUES (p_module, p_action, p_target_id, pkg_common.get_sys_date());
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_common.send_notification(
    p_channel VARCHAR,
    p_message VARCHAR
) AS $$
BEGIN
    INSERT INTO t_notifications(channel, message, sent_at)
    VALUES (p_channel, p_message, pkg_common.get_sys_date());
END;
$$ LANGUAGE plpgsql;
