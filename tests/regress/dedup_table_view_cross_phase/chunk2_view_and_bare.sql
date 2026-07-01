-- Chunk 2: View "bigfund.orders" + procedure p2 with bare "orders" reference.
-- Phase 1: qualified Table "bigfund.orders" → View "bigfund.orders" (merge)
-- Phase 2: bare Table "orders" → qualified Table "bigfund.orders" (merge)
-- The qualified Table is removed in Phase 1, then used as into_idx in
-- Phase 2, triggering petgraph panic without the fix.
CREATE VIEW bigfund.orders AS
SELECT * FROM x;

CREATE OR REPLACE PROCEDURE p2() AS $$
BEGIN
    SELECT * FROM orders;
END;
$$;
