CREATE OR REPLACE PROCEDURE pkg_report.generate_daily_report(
    p_date VARCHAR
) AS $$
BEGIN
    CALL pkg_order.get_order_detail(0);
    PERFORM pkg_payment.query_payment_status(0);
    INSERT INTO t_reports(type, content, generated_at)
    VALUES ('DAILY', p_date, pkg_common.get_sys_date());
    PERFORM pkg_common.log_operation('REPORT', 'DAILY', 0);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_report.generate_sales_report(
    p_start_date VARCHAR,
    p_end_date VARCHAR
) AS $$
BEGIN
    INSERT INTO t_reports(type, content, generated_at)
    VALUES ('SALES', p_start_date || '~' || p_end_date, pkg_common.get_sys_date());
    PERFORM pkg_common.log_operation('REPORT', 'SALES', 0);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_report.export_report_to_file(
    p_report_id BIGINT
) AS $$
BEGIN
    PERFORM pkg_common.log_operation('REPORT', 'EXPORT', p_report_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_report.cleanup_old_reports(
    p_days INT
) AS $$
BEGIN
    DELETE FROM t_reports WHERE generated_at < CURRENT_DATE - p_days;
    PERFORM pkg_common.log_operation('REPORT', 'CLEANUP', 0);
END;
$$ LANGUAGE plpgsql;
