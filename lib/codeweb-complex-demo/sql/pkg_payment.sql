CREATE OR REPLACE PROCEDURE pkg_payment.process_payment(
    p_order_id BIGINT,
    p_amount NUMERIC,
    p_method VARCHAR
) AS $$
DECLARE
    v_formatted VARCHAR;
BEGIN
    v_formatted := pkg_common.format_amount(p_amount);
    INSERT INTO t_payments(order_id, amount, method, status, paid_at)
    VALUES (p_order_id, p_amount, p_method, 'PAID', pkg_common.get_sys_date());
    PERFORM pkg_common.log_operation('PAYMENT', 'PROCESS', p_order_id);
    PERFORM pkg_common.send_notification('SMS', 'Payment processed');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_payment.refund_payment(
    p_order_id BIGINT
) AS $$
BEGIN
    UPDATE t_payments SET status = 'REFUNDED' WHERE order_id = p_order_id;
    PERFORM pkg_common.log_operation('PAYMENT', 'REFUND', p_order_id);
    PERFORM pkg_common.send_notification('EMAIL', 'Payment refunded');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION pkg_payment.query_payment_status(
    p_order_id BIGINT
) RETURNS VARCHAR AS $$
DECLARE
    v_status VARCHAR;
BEGIN
    SELECT status INTO v_status FROM t_payments WHERE order_id = p_order_id;
    RETURN COALESCE(v_status, 'NO_PAYMENT');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_payment.reconcile_payments(
    p_date VARCHAR
) AS $$
BEGIN
    INSERT INTO t_reconciliation(date, total_amount, total_count)
    SELECT p_date, SUM(amount), COUNT(*)
    FROM t_payments
    WHERE DATE(paid_at) = p_date::DATE AND status = 'PAID';
    PERFORM pkg_common.log_operation('PAYMENT', 'RECONCILE', 0);
END;
$$ LANGUAGE plpgsql;
