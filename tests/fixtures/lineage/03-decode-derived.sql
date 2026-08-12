-- ============================================================
-- 测试案例 03: DECODE/CASE 表达式变换 (Derived)
-- 来源: gh_temp.bs → out_trd_gh_jy.bs + V_DAT_INST_SECU_DEAL_INFO.quantity
-- 覆盖: DECODE 单层变换、嵌套 DECODE、CASE WHEN 多分支
-- ============================================================

-- 源表 1（交易数据）
CREATE TABLE raw_trade (
    account_id   VARCHAR(20),
    branch_code  VARCHAR(8),
    product_code VARCHAR(12),
    bs_flag      VARCHAR(2),         -- 原始买卖标志: 'B'/'S'/'G'/'H'
    trade_qty    NUMERIC(16,0),
    trade_price  NUMERIC(15,9),
    trade_amount NUMERIC(15,2)
);

-- 源表 2（操作类型字典）
CREATE TABLE operation_dict (
    operation_no  VARCHAR(10) NOT NULL,
    operation_name VARCHAR(50)
);

-- 源表 3（证券主数据）
CREATE TABLE security_master (
    product_code    VARCHAR(12) NOT NULL,
    market_code     VARCHAR(3),
    stock_category  VARCHAR(4),      -- 证券类别: '02'=国债, '06'=可转债, ...
    begin_date      VARCHAR(8),
    end_date        VARCHAR(8)
);

-- 目标表（标准化后）
CREATE TABLE normalized_trade (
    account_id   VARCHAR(20),
    branch_code  VARCHAR(8),
    product_code VARCHAR(12),
    -- 变换后的买卖标志: '1B'/'1S'/'0B'/'0S'/'0'
    bs_flag      VARCHAR(2),
    trade_qty    NUMERIC(18,8),
    trade_amount NUMERIC(15,3),
    -- 视图中的列
    adj_quantity NUMERIC(18,8),      -- 经 DECODE 调整后的数量
    market_code  VARCHAR(3)
);

-- ===========================================================
-- 场景 A: 简单 DECODE 变换
--   decode(bs, 'B', '1B', 'S', '1S', '0') → bs_flag
-- ===========================================================
INSERT INTO normalized_trade (
    account_id, branch_code, product_code, bs_flag, trade_qty, trade_amount
)
SELECT
    t.account_id,
    t.branch_code,
    t.product_code,
    DECODE(t.bs_flag, 'B', '1B', 'S', '1S', '0'),   -- 表达式变换
    t.trade_qty,
    t.trade_amount
FROM raw_trade t;

-- 预期:
--   raw_trade.bs_flag → normalized_trade.bs_flag [Derived: DECODE(bs_flag, 'B','1B','S','1S','0')]

-- ===========================================================
-- 场景 B: 嵌套 DECODE 变换（多层条件）
--   decode(operation_no,
--     '0601', t.qty / 100,
--     '0603', t.qty,
--     decode(market_code, '003', decode(stock_category, '02', t.qty/100, ...), t.qty))
-- ===========================================================
INSERT INTO normalized_trade (
    account_id, product_code, bs_flag, adj_quantity, market_code
)
SELECT
    t.account_id,
    t.product_code,
    t.bs_flag,
    DECODE(w.operation_no,
           '0601000001', t.trade_qty / 100,          -- 国债: 手→张
           '0603033001', t.trade_qty,                 -- 转债: 张
           '0603076001', t.trade_qty,                 -- 回购: 张
           DECODE(s.market_code,
                  '003', DECODE(s.stock_category,     -- 深圳市场
                                '02', t.trade_qty / 100,
                                '06', t.trade_qty / 100,
                                t.trade_qty),
                  '046', DECODE(s.stock_category,     -- 北京市场
                                '02', t.trade_qty / 100,
                                t.trade_qty),
                  t.trade_qty)) AS adj_quantity,
    s.market_code
FROM raw_trade t
JOIN security_master s ON t.product_code = s.product_code
JOIN operation_dict w ON 1 = 1   -- 简化: 实际通过 base_info 关联
WHERE t.trade_date BETWEEN s.begin_date AND s.end_date;

-- 预期:
--   adj_quantity 的血缘应包含:
--     raw_trade.trade_qty         [参与 DECODE 表达式]
--     operation_dict.operation_no  [参与 DECODE 条件判断]
--     security_master.market_code  [参与嵌套 DECODE 条件]
--     security_master.stock_category [参与嵌套 DECODE 条件]
--   变换类型: [Derived: nested DECODE(...)]

-- ===========================================================
-- 场景 C: CASE WHEN 多分支（佣金类型选择器）
-- ===========================================================
-- 源表（含多个佣金列）
CREATE TABLE trade_with_commission (
    account_id  VARCHAR(20),
    trade_date  VARCHAR(8),
    commission_type VARCHAR(2),      -- '11'/'12'/'21'/'22'/...
    -- 各类型佣金列
    yj_broker_a NUMERIC(20,8),       -- 佣金11
    yj_broker_b NUMERIC(20,8),       -- 佣金12
    yj_exch_a   NUMERIC(20,8),       -- 佣金21
    yj_exch_b   NUMERIC(20,8),       -- 佣金22
    yj_default  NUMERIC(20,8)        -- 默认佣金
);

CREATE TABLE trade_commission (
    account_id     VARCHAR(20),
    trade_date     VARCHAR(8),
    commission_amt NUMERIC(20,8)     -- 实际佣金 (根据 commission_type 选择不同源列)
);

INSERT INTO trade_commission (account_id, trade_date, commission_amt)
SELECT
    account_id,
    trade_date,
    CASE commission_type
        WHEN '11' THEN yj_broker_a
        WHEN '12' THEN yj_broker_b
        WHEN '21' THEN yj_exch_a
        WHEN '22' THEN yj_exch_b
        ELSE yj_default
    END
FROM trade_with_commission;

-- 预期:
--   trade_commission.commission_amt [Derived: CASE commission_type WHEN '11' THEN yj_broker_a ...]
--     源列: yj_broker_a, yj_broker_b, yj_exch_a, yj_exch_b, yj_default (全部分支)
--   注意: 这是 FAN-IN 血缘 —— 多个源列收敛到一个目标列
