-- Chunk 1: View query creates qualified Table "bigfund.orders"
-- without polluting table_index["orders"] (View query handler
-- lacks the bare-name insertion present in procedure-body handler).
CREATE VIEW v1 AS
SELECT * FROM bigfund.orders;
