-- ============================================================
-- Operator Tracking Regression Test
-- 验证 ANY / ALL / SOME / EXISTS / IN 的提取和查询
-- ============================================================

-- 1. ANY 操作符
CREATE OR REPLACE PROCEDURE proc_use_any() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM orders WHERE amount > ANY(SELECT amount FROM large_orders)) LOOP
        RAISE NOTICE 'order: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 2. ALL 操作符
CREATE OR REPLACE PROCEDURE proc_use_all() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM products WHERE price > ALL(SELECT price FROM discounted_products)) LOOP
        RAISE NOTICE 'product: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 3. EXISTS 谓词
CREATE OR REPLACE PROCEDURE proc_use_exists() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM customers c WHERE EXISTS(
        SELECT 1 FROM orders o WHERE o.customer_id = c.id AND o.status = 'active'
    )) LOOP
        RAISE NOTICE 'customer: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 4. IN 子查询
CREATE OR REPLACE PROCEDURE proc_use_in() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM orders WHERE customer_id IN (
        SELECT id FROM customers WHERE vip = true
    )) LOOP
        RAISE NOTICE 'vip order: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 5. NOT IN 子查询
CREATE OR REPLACE PROCEDURE proc_use_not_in() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM orders WHERE customer_id NOT IN (
        SELECT id FROM customers WHERE blacklisted = true
    )) LOOP
        RAISE NOTICE 'clean order: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 6. SOME 操作符 (和 ANY 分开追踪)
CREATE OR REPLACE PROCEDURE proc_use_some() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (SELECT * FROM orders WHERE amount = SOME(SELECT amount FROM large_orders)) LOOP
        RAISE NOTICE 'order: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- 7. 多个操作符混合使用
CREATE OR REPLACE PROCEDURE proc_mixed_ops() AS $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN (
        SELECT * FROM orders o
        WHERE o.amount > ANY(SELECT amount FROM large_orders)
          AND EXISTS(SELECT 1 FROM customers c WHERE c.id = o.customer_id)
          AND o.region IN (SELECT code FROM active_regions)
    ) LOOP
        RAISE NOTICE 'mixed: %', r.id;
    END LOOP;
END;
$$ LANGUAGE plpgsql;
