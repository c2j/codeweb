-- Scenario: trace-sql search across multiple procedures with different DML types.
-- Tests keyword-aware filtering: searching INSERT should NOT match SELECT/UPDATE/DELETE.

CREATE OR REPLACE PROCEDURE proc_insert_order(
    p_order_id   BIGINT,
    p_amount     DECIMAL(18,2),
    p_customer   VARCHAR(100)
) AS $$
BEGIN
    INSERT INTO t_orders (id, amount, customer_name, created_at, status)
    VALUES (p_order_id, p_amount, p_customer, now(), 'PENDING');
    COMMIT;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_update_order_status(
    p_order_id BIGINT,
    p_status   VARCHAR(20)
) AS $$
BEGIN
    UPDATE t_orders
    SET status = p_status, updated_at = now()
    WHERE id = p_order_id;
    COMMIT;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_delete_expired_orders() AS $$
BEGIN
    DELETE FROM t_orders
    WHERE status = 'EXPIRED' AND created_at < now() - INTERVAL '30 days';
    COMMIT;
END;
$$ LANGUAGE plpgsql;
/
