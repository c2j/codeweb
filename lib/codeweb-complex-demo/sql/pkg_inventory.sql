CREATE OR REPLACE PROCEDURE pkg_inventory.check_stock(
    p_product_id BIGINT,
    p_qty INT
) AS $$
DECLARE
    v_available INT;
BEGIN
    SELECT stock_qty INTO v_available FROM t_products WHERE id = p_product_id;
    IF v_available < p_qty THEN
        RAISE EXCEPTION 'Insufficient stock: % < %', v_available, p_qty;
    END IF;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_inventory.reserve_stock(
    p_product_id BIGINT,
    p_qty INT
) AS $$
BEGIN
    CALL pkg_inventory.check_stock(p_product_id, p_qty);
    UPDATE t_products SET stock_qty = stock_qty - p_qty WHERE id = p_product_id;
    INSERT INTO t_inventory_log(product_id, delta, reason)
    VALUES (p_product_id, -p_qty, 'RESERVE');
    PERFORM pkg_common.log_operation('INVENTORY', 'RESERVE', p_product_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_inventory.release_stock(
    p_product_id BIGINT,
    p_qty INT
) AS $$
BEGIN
    UPDATE t_products SET stock_qty = stock_qty + p_qty WHERE id = p_product_id;
    INSERT INTO t_inventory_log(product_id, delta, reason)
    VALUES (p_product_id, p_qty, 'RELEASE');
    PERFORM pkg_common.log_operation('INVENTORY', 'RELEASE', p_product_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_inventory.sync_from_supplier(
    p_supplier_id BIGINT
) AS $$
BEGIN
    INSERT INTO t_inventory_log(product_id, delta, reason)
    SELECT id, 100, 'SUPPLIER_SYNC'
    FROM t_products WHERE supplier_id = p_supplier_id AND active = true;
    PERFORM pkg_common.log_operation('INVENTORY', 'SUPPLIER_SYNC', p_supplier_id);
END;
$$ LANGUAGE plpgsql;
