CREATE OR REPLACE PROCEDURE process_order(p_id IN BIGINT) AS
BEGIN
    INSERT INTO order_log (order_id, action, processed_at)
    VALUES (p_id, 'processed', CURRENT_TIMESTAMP);
END;
/
