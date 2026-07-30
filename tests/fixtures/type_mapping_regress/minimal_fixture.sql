-- Minimal fixture for type mapping regression tests (issues #111, #116)
-- Contains: package, procedure, function, table, sequence, trigger, type

CREATE TABLE orders (
    id BIGINT PRIMARY KEY,
    amount DECIMAL(10,2)
);

CREATE SEQUENCE seq_order_id;

CREATE TYPE order_status AS ENUM ('pending', 'shipped');

CREATE OR REPLACE PACKAGE pkg_order_mgmt AS
    PROCEDURE create_order(p_amount DECIMAL);
    FUNCTION get_total(p_customer_id BIGINT) RETURN DECIMAL;
END pkg_order_mgmt;
/

CREATE OR REPLACE PACKAGE BODY pkg_order_mgmt AS
    PROCEDURE create_order(p_amount DECIMAL) IS
    BEGIN
        INSERT INTO orders(id, amount) VALUES (seq_order_id.NEXTVAL, p_amount);
    END create_order;

    FUNCTION get_total(p_customer_id BIGINT) RETURN DECIMAL IS
        v_total DECIMAL(10,2);
    BEGIN
        SELECT SUM(amount) INTO v_total FROM orders;
        RETURN v_total;
    END get_total;
END pkg_order_mgmt;
/
