CREATE OR REPLACE PACKAGE pkg AS
    PROCEDURE update_status(p_id IN BIGINT, p_status IN VARCHAR);
END;
/

CREATE OR REPLACE PACKAGE BODY pkg AS
    PROCEDURE update_status(p_id IN BIGINT, p_status IN VARCHAR) AS
    BEGIN
        UPDATE orders SET status = p_status WHERE id = p_id;
    END;
END;
/
