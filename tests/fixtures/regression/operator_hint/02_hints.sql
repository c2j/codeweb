-- ============================================================
-- Hint Tracking Regression Test
-- 验证 GaussDB optimizer hints 的提取和查询
-- ============================================================

-- 1. 单表扫描 hint
CREATE OR REPLACE PROCEDURE proc_hint_tablescan() AS $$
DECLARE
    r RECORD;
BEGIN
    SELECT /*+ tablescan(t1) */ * INTO r FROM orders t1 WHERE t1.id = 1;
END;
$$ LANGUAGE plpgsql;

-- 2. Join 方法 hint
CREATE OR REPLACE PROCEDURE proc_hint_hashjoin() AS $$
DECLARE
    r RECORD;
BEGIN
    SELECT /*+ hashjoin(t1 t2) */ t1.* INTO r
    FROM orders t1 JOIN customers t2 ON t1.customer_id = t2.id;
END;
$$ LANGUAGE plpgsql;

-- 3. 多个 hint 混合
CREATE OR REPLACE PROCEDURE proc_hint_multi() AS $$
DECLARE
    r RECORD;
BEGIN
    SELECT /*+ tablescan(t1) hashjoin(t1 t2) leading(t1 t2) */ t1.* INTO r
    FROM orders t1 JOIN customers t2 ON t1.customer_id = t2.id;
END;
$$ LANGUAGE plpgsql;

-- 4. 子查询 rewrite hint
CREATE OR REPLACE PROCEDURE proc_hint_rewrite() AS $$
DECLARE
    r RECORD;
BEGIN
    SELECT /*+ expand_sublink */ * INTO r
    FROM orders WHERE customer_id IN (SELECT id FROM customers WHERE vip = true);
END;
$$ LANGUAGE plpgsql;

-- 5. 带参数 hint (hint 名在 SelectStatement.hints 中存储，参数仅用于验证)
CREATE OR REPLACE PROCEDURE proc_hint_with_args() AS $$
DECLARE
    r RECORD;
BEGIN
    SELECT /*+ blockname(@sel$1 bn1) nestloop(t1 bn1) */ * INTO r
    FROM orders t1 WHERE t1.id = 1;
END;
$$ LANGUAGE plpgsql;
