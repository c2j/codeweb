-- ============================================================
-- 测试案例 10: INSERT 多源 UNION ALL (非视图)
-- 来源: out_trd_gh_jy 从 6+ 源表 UNION ALL INSERT
-- 覆盖: 多表 UNION ALL 插入、每个分支独立的列映射、表级多源血缘
-- ============================================================

-- 三种不同格式的原始交易数据源
CREATE TABLE trade_source_exchange (
    account        VARCHAR(20),
    broker_id      VARCHAR(8),
    product        VARCHAR(12),
    trade_ref      VARCHAR(16),
    bs_flag        VARCHAR(2),
    quantity       NUMERIC(15,0),
    price          NUMERIC(11,3),
    amount         NUMERIC(16,2),
    trade_date     VARCHAR(8),
    source_tag     VARCHAR(1)
);

CREATE TABLE trade_source_otc (
    trader_code   VARCHAR(20),
    dealer_code   VARCHAR(8),
    security_code VARCHAR(12),
    trade_no      VARCHAR(16),
    direction     VARCHAR(2),
    volume        NUMERIC(15,0),
    deal_price    NUMERIC(11,3),
    deal_amount   NUMERIC(16,2),
    deal_date     VARCHAR(8),
    source        VARCHAR(1)
);

CREATE TABLE trade_source_block (
    acct_id       VARCHAR(20),
    dealer        VARCHAR(8),
    secu_id       VARCHAR(12),
    exec_id       VARCHAR(16),
    side          VARCHAR(2),
    exec_qty      NUMERIC(15,0),
    exec_price    NUMERIC(11,3),
    exec_amount   NUMERIC(16,2),
    exec_date     VARCHAR(8),
    src           VARCHAR(1)
);

-- 统一格式的目标表
CREATE TABLE trade_consolidated (
    account_id   VARCHAR(20),
    branch_id    VARCHAR(8),
    product_id   VARCHAR(12),
    trade_id     VARCHAR(16),
    bs_side      VARCHAR(2),
    trade_qty    NUMERIC(15,0),
    trade_price  NUMERIC(11,3),
    trade_amount NUMERIC(16,2),
    trade_date   VARCHAR(8),
    source_tag   VARCHAR(1)
);

-- 多源 UNION ALL + 每源独立列映射
INSERT INTO trade_consolidated (
    account_id, branch_id, product_id, trade_id,
    bs_side, trade_qty, trade_price, trade_amount, trade_date, source_tag
)
-- 源1: 交易所数据
SELECT
    account,                                      -- account → account_id
    broker_id,                                    -- broker_id → branch_id
    product,                                      -- product → product_id
    trade_ref,                                    -- trade_ref → trade_id
    DECODE(bs_flag, 'B', '1B', 'S', '1S', '0'),   -- bs_flag → bs_side [DECODE]
    quantity,                                     -- quantity → trade_qty
    price,                                        -- price → trade_price
    amount,                                       -- amount → trade_amount
    trade_date,
    'E'                                           -- 硬编码: 交易所
FROM trade_source_exchange
WHERE trade_date = '20250715'

UNION ALL

-- 源2: 场外交易
SELECT
    trader_code,                                  -- trader_code → account_id
    dealer_code,                                  -- dealer_code → dealer_id
    security_code,                                -- security_code → product_id
    trade_no,                                     -- trade_no → trade_id
    DECODE(direction, 'B', '0B', '0S'),            -- direction → bs_side [DECODE, 不同映射]
    volume / 10,                                  -- volume/10 → trade_qty [数量单位换算]
    deal_price,                                   -- deal_price → trade_price
    deal_amount * 100,                            -- deal_amount*100 → trade_amount [金额单位换算]
    deal_date,
    'O'                                           -- 硬编码: 场外
FROM trade_source_otc
WHERE deal_date = '20250715'

UNION ALL

-- 源3: 大宗交易
SELECT
    acct_id,                                      -- acct_id → account_id
    dealer,                                       -- dealer → branch_id
    secu_id,                                      -- secu_id → product_id
    exec_id,                                      -- exec_id → trade_id
    side,                                         -- side → bs_side [DataFlow, 无变换]
    exec_qty / 100,                               -- exec_qty/100 → trade_qty [数量单位换算]
    exec_price,
    exec_amount,
    exec_date,
    'B'                                           -- 硬编码: 大宗
FROM trade_source_block
WHERE exec_date = '20250715';

-- 预期:
--   codeweb lineage trade_consolidated --direction upstream (表级)
--   trade_consolidated [table, W: INSERT UNION ALL]
--     ├── trade_source_exchange [R]
--     ├── trade_source_otc     [R]
--     └── trade_source_block   [R]
--
--   codeweb lineage trade_consolidated.bs_side --direction upstream (列级)
--   trade_consolidated.bs_side [UNION ALL, 3 sources]
--     ├── trade_source_exchange.bs_flag  [Derived: DECODE(bs_flag, 'B','1B','S','1S','0')]
--     ├── trade_source_otc.direction     [Derived: DECODE(direction, 'B','0B','0S')]
--     └── trade_source_block.side        [DataFlow]
--
--   codeweb lineage trade_consolidated.trade_qty --direction upstream
--   trade_consolidated.trade_qty [UNION ALL, 3 sources]
--     ├── trade_source_exchange.quantity  [DataFlow]
--     ├── trade_source_otc.volume         [Derived: volume / 10]
--     └── trade_source_block.exec_qty     [Derived: exec_qty / 100]
--
-- 关键断言:
--   1. 同一目标表/列对应多个源表/列 (多源 FAN-IN)
--   2. 不同源的同一目标列变换方式不同 (bs_side: DECODE vs DECODE vs DataFlow)
--   3. 数量/金额在不同源可能有不同的单位换算
