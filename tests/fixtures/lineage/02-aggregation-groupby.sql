-- ============================================================
-- 测试案例 02: 聚合 + GROUP BY (OLAP 核心场景)
-- 来源: jsmx_temp → dat_trd_qfii_chinaclear (PKG_SPLIT_TRADE_STEP2)
-- 覆盖: SUM 聚合函数 + GROUP BY 维度键 + 聚合度量区分
-- ============================================================

-- 源表（明细数据）
CREATE TABLE trade_detail (
    account_id   VARCHAR(20),            -- GROUP BY 键 1
    branch_code  VARCHAR(8),             -- GROUP BY 键 2
    trade_date   VARCHAR(8),             -- GROUP BY 键 3
    bs_flag      VARCHAR(2),             -- GROUP BY 键 4
    product_code VARCHAR(12),            -- GROUP BY 键 5
    quantity     NUMERIC(15,2),          -- 聚合度量
    amount       NUMERIC(15,3),          -- 聚合度量
    fee_broker   NUMERIC(10,2),          -- 聚合度量: 佣金
    fee_transfer NUMERIC(10,2),          -- 聚合度量: 过户费
    fee_manage   NUMERIC(10,2),          -- 聚合度量: 管理费
    fee_settle   NUMERIC(10,2),          -- 聚合度量: 结算费
    market_code  VARCHAR(3)              -- 过滤条件使用
);

-- 目标表（按基金+席位+证券聚合后）
CREATE TABLE trade_summary (
    account_id     VARCHAR(20) NOT NULL,  -- GROUP BY 键
    branch_code    VARCHAR(8)  NOT NULL,  -- GROUP BY 键
    trade_date     VARCHAR(8)  NOT NULL,  -- GROUP BY 键
    bs_flag        VARCHAR(2)  NOT NULL,  -- GROUP BY 键
    product_code   VARCHAR(12) NOT NULL,  -- GROUP BY 键
    total_qty      NUMERIC(15,2),         -- SUM(quantity)
    total_amount   NUMERIC(15,3),         -- SUM(amount)
    total_broker   NUMERIC(10,2),         -- SUM(fee_broker)
    total_transfer NUMERIC(10,2),         -- SUM(fee_transfer)
    total_manage   NUMERIC(10,2),         -- SUM(fee_manage)
    total_settle   NUMERIC(10,2),         -- SUM(fee_settle)
    row_count      NUMERIC(10,0),         -- COUNT(1)
    market_code    VARCHAR(3)             -- 硬编码
);

-- INSERT SELECT with SUM aggregation and GROUP BY
INSERT INTO trade_summary (
    account_id, branch_code, trade_date, bs_flag, product_code,
    total_qty, total_amount, total_broker, total_transfer, total_manage, total_settle,
    row_count, market_code
)
SELECT
    t.account_id,                                    -- GROUP BY 键 → DataFlow
    t.branch_code,                                   -- GROUP BY 键 → DataFlow
    t.trade_date,                                    -- GROUP BY 键 → DataFlow
    t.bs_flag,                                       -- GROUP BY 键 → DataFlow
    t.product_code,                                  -- GROUP BY 键 → DataFlow
    SUM(ABS(t.quantity)),                            -- 聚合 → Aggregated: SUM
    SUM(ABS(t.amount)),                              -- 聚合 → Aggregated: SUM
    SUM(ABS(t.fee_broker)),                          -- 聚合 → Aggregated: SUM
    SUM(ABS(t.fee_transfer)),                        -- 聚合 → Aggregated: SUM
    SUM(ABS(t.fee_manage)),                          -- 聚合 → Aggregated: SUM
    SUM(ABS(t.fee_settle)),                          -- 聚合 → Aggregated: SUM
    COUNT(1),                                        -- 聚合 → Aggregated: COUNT
    '001'                                             -- 硬编码常量
FROM trade_detail t
WHERE t.trade_date = '20250715'
  AND t.market_code = '01'
  AND t.bs_flag IN ('B', 'S')
GROUP BY t.account_id, t.branch_code, t.trade_date, t.bs_flag, t.product_code;

-- 预期血缘输出:
--   codeweb lineage trade_summary.total_qty --direction upstream
--   trade_summary.total_qty [Aggregated: SUM, GROUP BY account_id,branch_code,trade_date,bs_flag,product_code]
--     └── trade_detail.quantity [DataFlow]
--
--   codeweb lineage trade_summary.account_id --direction upstream
--   trade_summary.account_id [GROUP BY key]
--     └── trade_detail.account_id [DataFlow]
--
-- 关键断言: GROUP BY 键和聚合度量的血缘类型不同
--   - account_id: 标注 "GROUP BY key" 而非 "Aggregated"
--   - total_qty:  标注 "Aggregated: SUM"
