-- ============================================================
-- 测试案例 12: 自引用 UPDATE 列级血缘
-- 来源: out_trd_gh_jy UPDATE cjsl = cjsl * trade_unit
--        out_trd_gh_zyk UPDATE bs = 'Z' || bs
-- 覆盖: UPDATE SET 列级血缘、自引用列变换、条件更新
-- ============================================================

-- ============================================
-- 场景 A: UPDATE SET with self-referencing arithmetic
--   cjsl = cjsl * trade_unit
-- ============================================
CREATE TABLE bond_positions (
    account_id   VARCHAR(20),
    product_code VARCHAR(12),
    trade_date   VARCHAR(8),
    position_qty NUMERIC(15,2),       -- 原数量
    unit_size    NUMERIC(5,0) DEFAULT 1,  -- 每手单位
    trade_amount NUMERIC(15,3),
    updated_flag VARCHAR(1) DEFAULT '0'
);

-- 插入数据
INSERT INTO bond_positions VALUES
    ('A001', 'BC001', '20250715', 1000, 1,   100000.00, '0'),
    ('A001', 'BC002', '20250715', 500,  10,  50000.00,  '0'),
    ('A002', 'BC003', '20250715', 200,  100, 20000.00,  '0');

-- UPDATE: position_qty 由自身和 unit_size 列计算
UPDATE bond_positions t
   SET t.position_qty = t.position_qty * t.unit_size,     -- 自引用变换
       t.updated_flag = '1'                                -- 标志更新
 WHERE t.trade_amount >= 0
   AND t.unit_size > 1;

-- 预期:
--   bond_positions.position_qty (新值) [Derived: position_qty * unit_size]
--     ├── bond_positions.position_qty (原值) [DataFlow, 自引用]
--     └── bond_positions.unit_size [DataFlow]
--   bond_positions.updated_flag [Literal: '1']

-- ============================================
-- 场景 B: UPDATE SET with string concatenation
--   bs = 'Z' || bs
-- ============================================
CREATE TABLE pledge_records (
    account_id   VARCHAR(20),
    product_code VARCHAR(12),
    trade_date   VARCHAR(8),
    bs_flag      VARCHAR(2),           -- 买卖标志: 'B', 'S'
    pledge_mark  VARCHAR(1) DEFAULT '0'
);

INSERT INTO pledge_records VALUES
    ('A001', 'P001', '20250715', 'B', '0'),
    ('A001', 'P002', '20250715', 'S', '0');

-- UPDATE: 质押标记拼接
UPDATE pledge_records t
   SET t.bs_flag = 'Z' || t.bs_flag        -- 字符串拼接: 'B'→'ZB', 'S'→'ZS'
 WHERE t.bs_flag IN ('B', 'S');

-- 预期:
--   pledge_records.bs_flag (新值) [Derived: 'Z' || bs_flag]
--     └── pledge_records.bs_flag (原值) [DataFlow, 自引用]

-- ============================================
-- 场景 C: UPDATE SET with multi-column expression
--   amount = qty * price + qty * fee_rate
-- ============================================
CREATE TABLE fee_adjustment (
    account_id   VARCHAR(20),
    trade_qty    NUMERIC(15,0),
    trade_price  NUMERIC(10,3),
    fee_rate     NUMERIC(8,6),
    trade_amount NUMERIC(15,3),
    adjusted     VARCHAR(1) DEFAULT '0'
);

INSERT INTO fee_adjustment VALUES
    ('A001', 1000, 10.500, 0.001000, 0, '0'),
    ('A002', 500,  20.300, 0.000500, 0, '0');

-- UPDATE: 多列参与计算
UPDATE fee_adjustment t
   SET t.trade_amount = t.trade_qty * t.trade_price +
                         t.trade_qty * t.fee_rate,           -- 多列表达式
       t.adjusted     = '1'
 WHERE t.adjusted = '0';

-- 预期:
--   fee_adjustment.trade_amount (新值) [Derived: qty*price + qty*fee_rate]
--     ├── fee_adjustment.trade_qty [DataFlow, 自引用]
--     ├── fee_adjustment.trade_price [DataFlow, 自引用]
--     └── fee_adjustment.fee_rate [DataFlow, 自引用]

-- ============================================
-- 场景 D: Existence-check then UPDATE (UPSERT pattern)
-- ============================================
CREATE TABLE upsert_target (
    account_id   VARCHAR(20) NOT NULL,
    trade_date   VARCHAR(8) NOT NULL,
    product_code VARCHAR(12) NOT NULL,
    trade_qty    NUMERIC(15,0),
    trade_amount NUMERIC(15,3),
    version      NUMERIC(5,0) DEFAULT 1,
    PRIMARY KEY (account_id, trade_date, product_code)
);

CREATE TABLE upsert_source (
    account_id   VARCHAR(20),
    trade_date   VARCHAR(8),
    product_code VARCHAR(12),
    new_qty      NUMERIC(15,0),
    new_amount   NUMERIC(15,3)
);

INSERT INTO upsert_source VALUES ('A001', '20250715', 'P001', 500, 50000.00);
INSERT INTO upsert_target VALUES ('A001', '20250715', 'P001', 200, 20000.00, 1);

-- UPSERT 逻辑: 先检查是否存在
DECLARE
    v_count NUMBER;
BEGIN
    SELECT COUNT(1) INTO v_count
    FROM upsert_target t
    WHERE t.account_id = 'A001'
      AND t.trade_date = '20250715'
      AND t.product_code = 'P001';

    IF v_count > 0 THEN
        -- 存在则 UPDATE
        UPDATE upsert_target
           SET trade_qty    = (SELECT new_qty FROM upsert_source WHERE account_id = 'A001'),
               trade_amount = (SELECT new_amount FROM upsert_source WHERE account_id = 'A001'),
               version      = version + 1
         WHERE account_id = 'A001'
           AND trade_date = '20250715'
           AND product_code = 'P001';
    ELSE
        -- 不存在则 INSERT
        INSERT INTO upsert_target (account_id, trade_date, product_code, trade_qty, trade_amount)
        SELECT account_id, trade_date, product_code, new_qty, new_amount
        FROM upsert_source;
    END IF;
    COMMIT;
END;
/

-- 预期:
--   UPSERT 目标列 trade_qty 有两种血缘路径:
--     UPDATE 路径: upsert_source.new_qty → upsert_target.trade_qty [DataFlow]
--     INSERT 路径: upsert_source.new_qty → upsert_target.trade_qty [DataFlow]
--   version 列:
--     upsert_target.version (原值) + 常量1 → upsert_target.version (新值) [Derived: version+1]
