CREATE OR REPLACE PACKAGE pkg AS
    FUNCTION calc_total(p_order_id IN BIGINT) RETURN DECIMAL;
    FUNCTION get_last_order RETURN BIGINT;
END;
/

CREATE OR REPLACE PACKAGE BODY pkg AS
    FUNCTION calc_total(p_order_id IN BIGINT) RETURN DECIMAL AS
        v_total DECIMAL := 0;
    BEGIN
        SELECT SUM(amount) INTO v_total FROM orders WHERE id = p_order_id;
        RETURN v_total;
    END;

    FUNCTION get_last_order RETURN BIGINT AS
        v_id BIGINT;
    BEGIN
        SELECT MAX(id) INTO v_id FROM orders;
        RETURN v_id;
    END;
END;
/
