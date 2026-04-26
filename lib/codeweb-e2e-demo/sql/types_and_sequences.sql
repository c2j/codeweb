CREATE TYPE address_t AS (
    street  VARCHAR(200),
    city    VARCHAR(100),
    zip     VARCHAR(20)
);

CREATE TYPE user_status_t AS ENUM ('ACTIVE', 'INACTIVE', 'SUSPENDED');

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.update_address(
    p_user_id BIGINT,
    p_addr    address_t
) AS $$
BEGIN
    UPDATE t_users SET
        street = p_addr.street,
        city   = p_addr.city,
        zip    = p_addr.zip
    WHERE id = p_user_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION pkg_user_mgmt.get_address(
    p_user_id BIGINT
) RETURNS address_t AS $$
DECLARE
    v_addr address_t;
BEGIN
    SELECT street, city, zip INTO v_addr
    FROM t_users WHERE id = p_user_id;
    RETURN v_addr;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_user_mgmt.set_status(
    p_user_id BIGINT,
    p_status  user_status_t
) AS $$
BEGIN
    UPDATE t_users SET status = p_status::TEXT WHERE id = p_user_id;
END;
$$ LANGUAGE plpgsql;

-- SEQUENCE definitions and usage

CREATE SEQUENCE user_id_seq START WITH 1 INCREMENT BY 1 CACHE 20;

CREATE SEQUENCE order_id_seq START WITH 1000 INCREMENT BY 1;

CREATE OR REPLACE FUNCTION pkg_user_mgmt.next_user_id() RETURNS BIGINT AS $$
BEGIN
    RETURN nextval('user_id_seq');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_order.create_order(
    p_user_id BIGINT,
    p_product VARCHAR
) AS $$
BEGIN
    INSERT INTO t_orders(id, user_id, product, status)
    VALUES(order_id_seq.NEXTVAL, p_user_id, p_product, 'PENDING');
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION pkg_order.current_order_id() RETURNS BIGINT AS $$
BEGIN
    RETURN currval('order_id_seq');
END;
$$ LANGUAGE plpgsql;

-- INDEX definitions

CREATE UNIQUE INDEX idx_users_email ON t_users(email);

CREATE INDEX idx_orders_user_status ON t_orders(user_id, status);

-- MATERIALIZED VIEW

CREATE MATERIALIZED VIEW mv_user_order_count AS
SELECT u.id AS user_id, u.name, COUNT(o.id) AS order_count
FROM t_users u
LEFT JOIN t_orders o ON u.id = o.user_id
GROUP BY u.id, u.name
WITH DATA;

-- SYNONYM

CREATE OR REPLACE PROCEDURE remote_api.do_work(p_task VARCHAR) AS $$
BEGIN
    INSERT INTO t_audit_log(action, detail) VALUES('REMOTE_WORK', p_task);
END;
$$ LANGUAGE plpgsql;

CREATE SYNONYM syn_do_work FOR remote_api.do_work;

-- EVENT (openGauss scheduled job)

CREATE EVENT evt_nightly_cleanup
ON SCHEDULE EVERY 1 DAY
STARTS '2026-01-01 02:00:00'
DO CALL pkg_audit.purge_old_logs(30);
