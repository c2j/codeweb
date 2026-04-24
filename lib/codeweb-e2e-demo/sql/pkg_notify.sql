CREATE OR REPLACE FUNCTION pkg_notify.send_event(
    p_event_type VARCHAR,
    p_ref_id     BIGINT
) RETURNS BOOLEAN AS $$
BEGIN
    INSERT INTO t_events(event_type, ref_id, created_at)
    VALUES(p_event_type, p_ref_id, now());
    PERFORM pkg_notify.append_log(p_event_type);
    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION pkg_notify.append_log(
    p_event_type VARCHAR
) RETURNS VOID AS $$
BEGIN
    INSERT INTO t_notify_log(event_type, created_at)
    VALUES(p_event_type, now());
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_notify.broadcast(
    p_message VARCHAR
) AS $$
BEGIN
    PERFORM pkg_notify.send_event('BROADCAST', 0);
    INSERT INTO t_notifications(message) VALUES(p_message);
END;
$$ LANGUAGE plpgsql;
