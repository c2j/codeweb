-- Case 07: Mixed package + cross-schema
-- Target: table `public.orders`
-- Graph:
--   orders ←TableAccess-- pkg_order.process_order (package procedure)
--   pkg_order.process_order ←DirectCall-- pkg_report.generate_report
--   orders ←TableAccess-- finance.calc_revenue (cross-schema)

CREATE TABLE orders (
    id INT PRIMARY KEY,
    amount NUMERIC(10, 2),
    region VARCHAR(50)
);

CREATE OR REPLACE PACKAGE pkg_order AS
    PROCEDURE process_order(order_id INT);
    PROCEDURE cancel_order(order_id INT);
END pkg_order;
/

CREATE OR REPLACE PACKAGE BODY pkg_order AS
    PROCEDURE process_order(order_id INT) AS
    BEGIN
        UPDATE orders SET amount = amount * 1.1 WHERE id = order_id;
    END;

    PROCEDURE cancel_order(order_id INT) AS
    BEGIN
        DELETE FROM orders WHERE id = order_id;
    END;
END pkg_order;
/

CREATE OR REPLACE PACKAGE pkg_report AS
    PROCEDURE generate_report(region VARCHAR);
END pkg_report;
/

CREATE OR REPLACE PACKAGE BODY pkg_report AS
    PROCEDURE generate_report(region VARCHAR) AS
    BEGIN
        pkg_order.process_order(0);
        SELECT SUM(amount) FROM orders WHERE region = generate_report.region;
    END;
END pkg_report;
/

-- Cross-schema procedure accessing orders
CREATE OR REPLACE PROCEDURE finance.calc_revenue()
AS
    v_total NUMERIC;
BEGIN
    SELECT SUM(amount) INTO v_total FROM orders;
END;
/
