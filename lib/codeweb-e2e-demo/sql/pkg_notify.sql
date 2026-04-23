-- ============================================================
-- Package: pkg_notify — event notification procedures
-- ============================================================

CREATE OR REPLACE FUNCTION pkg_notify.send_event(
    p_event_type VARCHAR,
    p_ref_id     BIGINT
) RETURNS BOOLEAN AS $$
BEGIN
    INSERT INTO t_events(event_type, ref_id, created_at)
    VALUES(p_event_type, p_ref_id, now());
    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_notify.broadcast(
    p_message VARCHAR
) AS $$
BEGIN
    INSERT INTO t_notifications(message) VALUES(p_message);
END;
$$ LANGUAGE plpgsql;
