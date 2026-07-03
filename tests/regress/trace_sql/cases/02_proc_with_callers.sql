CREATE OR REPLACE PROCEDURE proc_process_order(p_order_id BIGINT) AS $$
DECLARE
    v_count INT;
BEGIN
    SELECT count(*) INTO v_count FROM t_orders WHERE id = p_order_id;

    IF v_count > 0 THEN
        UPDATE t_orders SET status = 'PROCESSING' WHERE id = p_order_id;

        INSERT INTO t_order_audit (order_id, action, executed_at)
        VALUES (p_order_id, 'process', now());
    END IF;

    COMMIT;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_batch_process_orders() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN SELECT id FROM t_orders WHERE status = 'PENDING' LOOP
        CALL proc_process_order(r.id);
    END LOOP;
    COMMIT;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_daily_cleanup() AS $$
BEGIN
    CALL proc_delete_expired_orders();
    INSERT INTO t_cleanup_log (task, executed_at) VALUES ('daily_cleanup', now());
    COMMIT;
END;
$$ LANGUAGE plpgsql;
/
