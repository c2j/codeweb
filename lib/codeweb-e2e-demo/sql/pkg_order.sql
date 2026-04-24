CREATE OR REPLACE PROCEDURE pkg_order.close_order(
    p_order_id BIGINT
) AS $$
BEGIN
    UPDATE t_orders SET status = 'CLOSED' WHERE id = p_order_id;
    PERFORM pkg_notify.send_event('ORDER_CLOSED', p_order_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.cancel_order(
    p_order_id BIGINT
) AS $$
BEGIN
    CALL pkg_order.close_order(p_order_id);
    PERFORM pkg_audit.log_detail('ORDER_CANCEL', p_order_id::TEXT);
    UPDATE t_orders SET status = 'CANCELLED' WHERE id = p_order_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.batch_cancel_by_user(
    p_user_id BIGINT
) AS $$
BEGIN
    CALL pkg_order.cancel_order(0);
    CALL pkg_user_mgmt.deactivate_user(p_user_id);
    PERFORM pkg_notify.send_event('BATCH_CANCEL', p_user_id);
END;
$$ LANGUAGE plpgsql;
