CREATE OR REPLACE PACKAGE pkg AS
    PROCEDURE get_user(p_id IN BIGINT, p_name OUT VARCHAR);
END;
/

CREATE OR REPLACE PACKAGE BODY pkg AS
    PROCEDURE get_user(p_id IN BIGINT, p_name OUT VARCHAR) AS
    BEGIN
        SELECT name INTO p_name FROM users WHERE id = p_id;
    END;
END;
/
