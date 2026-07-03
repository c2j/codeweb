CREATE OR REPLACE PROCEDURE proc_generate_monthly_report(
    p_year  INT,
    p_month INT
) AS $$
DECLARE
    v_total_orders    INT;
    v_total_revenue   DECIMAL(18,2);
    v_top_customer    VARCHAR(100);
BEGIN
    SELECT count(*), COALESCE(sum(o.amount), 0)
    INTO v_total_orders, v_total_revenue
    FROM t_orders o
    WHERE EXTRACT(YEAR FROM o.created_at) = p_year
      AND EXTRACT(MONTH FROM o.created_at) = p_month
      AND o.status IN ('COMPLETED', 'SHIPPED');

    SELECT c.customer_name
    INTO v_top_customer
    FROM t_orders o
    JOIN t_customers c ON o.customer_id = c.id
    WHERE EXTRACT(YEAR FROM o.created_at) = p_year
      AND EXTRACT(MONTH FROM o.created_at) = p_month
    GROUP BY c.customer_name
    ORDER BY sum(o.amount) DESC
    LIMIT 1;

    INSERT INTO t_monthly_reports
        (report_year, report_month, total_orders, total_revenue, top_customer, generated_at)
    VALUES
        (p_year, p_month, v_total_orders, v_total_revenue, v_top_customer, now());

    COMMIT;
END;
$$ LANGUAGE plpgsql;
/
