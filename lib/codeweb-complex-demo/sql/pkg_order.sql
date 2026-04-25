CREATE OR REPLACE PROCEDURE pkg_order.create_order(
    p_user_id BIGINT,
    p_product_id BIGINT,
    p_qty INT
) AS $$
BEGIN
    CALL pkg_inventory.reserve_stock(p_product_id, p_qty);
    INSERT INTO t_orders(user_id, product_id, qty, status, created_at)
    VALUES (p_user_id, p_product_id, p_qty, 'CREATED', pkg_common.get_sys_date());
    PERFORM pkg_common.log_operation('ORDER', 'CREATE', 0);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.cancel_order(
    p_order_id BIGINT
) AS $$
DECLARE
    v_product_id BIGINT;
    v_qty INT;
BEGIN
    SELECT product_id, qty INTO v_product_id, v_qty
    FROM t_orders WHERE id = p_order_id;

    CALL pkg_inventory.release_stock(v_product_id, v_qty);
    UPDATE t_orders SET status = 'CANCELLED' WHERE id = p_order_id;
    PERFORM pkg_common.log_operation('ORDER', 'CANCEL', p_order_id);
    PERFORM pkg_common.send_notification('SMS', 'Order cancelled');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.get_order_detail(
    p_order_id BIGINT
) AS $$
BEGIN
    SELECT o.*, p.name as product_name
    FROM t_orders o
    JOIN t_products p ON o.product_id = p.id
    WHERE o.id = p_order_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.batch_create_orders(
    p_user_id BIGINT,
    p_items VARCHAR
) AS $$
BEGIN
    CALL pkg_order.create_order(p_user_id, 0, 1);
    PERFORM pkg_common.log_operation('ORDER', 'BATCH_CREATE', p_user_id);
    PERFORM pkg_common.send_notification('EMAIL', 'Batch order created');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.complete_order(
    p_order_id BIGINT
) AS $$
BEGIN
    UPDATE t_orders SET status = 'COMPLETED' WHERE id = p_order_id;
    PERFORM pkg_common.log_operation('ORDER', 'COMPLETE', p_order_id);
    PERFORM pkg_common.send_notification('EMAIL', 'Order completed');
END;
$$ LANGUAGE plpgsql;
