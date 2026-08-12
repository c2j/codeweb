-- ============================================================
-- 测试案例 08: 多层管道 (端到端列追踪)
-- 来源: jsmx_temp → out_trd_gh_jy → mid_yjqs_detail → dat_fund_cjqs → dat_inst_fund_cjqs
-- 覆盖: 4跳列追踪、每跳变换类型不同、depth 控制
-- ============================================================

-- Layer 1: 原始输入
CREATE TABLE src_raw (
    account_id   VARCHAR(20) NOT NULL,
    branch_code  VARCHAR(8),
    trade_date   VARCHAR(8)  NOT NULL,
    bs_flag      VARCHAR(2),
    product_code VARCHAR(12),
    trade_qty    NUMERIC(15,2),
    trade_amount NUMERIC(15,3),
    fee_yhs      NUMERIC(10,2),      -- 印花税
    fee_jsf      NUMERIC(10,2),      -- 经手费
    fee_zgf      NUMERIC(10,2),      -- 证管费
    fee_ghf      NUMERIC(10,2),      -- 过户费
    market_code  VARCHAR(3)
);

-- Layer 2: 标准化中间表 (跳1: src_raw → std_intermediate)
CREATE TABLE std_intermediate (
    account_id   VARCHAR(20),
    branch_code  VARCHAR(8),
    trade_date   VARCHAR(8),
    bs_flag      VARCHAR(2),
    product_code VARCHAR(12),
    trade_qty    NUMERIC(15,2),
    trade_amount NUMERIC(15,2),
    fee_yhs      NUMERIC(10,2),
    fee_jsf      NUMERIC(10,2),
    fee_zgf      NUMERIC(10,2),
    fee_ghf      NUMERIC(10,2),
    stock_kind   VARCHAR(4),         -- 新增列: 从字典表查找
    trade_unit   NUMERIC(5,0)        -- 新增列: 从字典表查找
);

-- 跳1: INSERT with LOOKUP join
INSERT INTO std_intermediate (
    account_id, branch_code, trade_date, bs_flag, product_code,
    trade_qty, trade_amount, fee_yhs, fee_jsf, fee_zgf, fee_ghf,
    stock_kind, trade_unit
)
SELECT
    t.account_id,
    t.branch_code,
    t.trade_date,
    DECODE(t.bs_flag, 'B', '1B', 'S', '1S', '0'),  -- 变换1: bs_flag DECODE
    t.product_code,
    t.trade_qty,
    ABS(t.trade_amount),                             -- 变换2: 取绝对值
    t.fee_yhs,
    t.fee_jsf,
    t.fee_zgf,
    t.fee_ghf,
    '0100',                                          -- stock_kind 硬编码 (简化)
    1                                                -- trade_unit 硬编码 (简化)
FROM src_raw t
WHERE t.trade_date = '20250715';

-- 跳1 额外 UPDATE: trade_qty 乘 trade_unit
UPDATE std_intermediate
   SET trade_qty = trade_qty * trade_unit
 WHERE trade_amount >= 0
   AND trade_unit > 1;

-- Layer 3: 基金级聚合 (跳2: std_intermediate → fund_aggregation)
CREATE TABLE fund_aggregation (
    fund_code      VARCHAR(8)  NOT NULL,
    branch_code    VARCHAR(8)  NOT NULL,
    product_code   VARCHAR(12) NOT NULL,
    bs_flag        VARCHAR(2)  NOT NULL,
    trade_date     VARCHAR(8)  NOT NULL,
    stock_kind     VARCHAR(4)  NOT NULL,
    total_qty      NUMERIC(15,2),      -- 聚合度量
    total_amount   NUMERIC(18,3),      -- 聚合度量
    total_yhs      NUMERIC(10,2),      -- 聚合度量
    total_jsf      NUMERIC(10,2),      -- 聚合度量
    total_zgf      NUMERIC(10,2),      -- 聚合度量
    total_ghf      NUMERIC(10,2),      -- 聚合度量
    calc_cost      NUMERIC(18,3)        -- 计算列: mrcb
);

-- 跳2: 按基金+证券聚合
INSERT INTO fund_aggregation (
    fund_code, branch_code, product_code, bs_flag, trade_date, stock_kind,
    total_qty, total_amount, total_yhs, total_jsf, total_zgf, total_ghf, calc_cost
)
SELECT
    'F001',                                          -- fund_code 硬编码 (简化)
    branch_code,
    product_code,
    bs_flag,
    trade_date,
    stock_kind,
    SUM(trade_qty),                                  -- 聚合: SUM
    SUM(trade_amount),                               -- 聚合: SUM
    SUM(fee_yhs),                                    -- 聚合: SUM
    SUM(fee_jsf),                                    -- 聚合: SUM
    SUM(fee_zgf),                                    -- 聚合: SUM
    SUM(fee_ghf),                                    -- 聚合: SUM
    SUM(trade_amount) +                               -- 计算: amount + ...
        CASE bs_flag WHEN '1' THEN 1 ELSE -1 END *    -- ...方向符号 *
        (SUM(fee_yhs) + SUM(fee_jsf) + SUM(fee_zgf) + SUM(fee_ghf))  -- ...费用合计
FROM std_intermediate
GROUP BY branch_code, product_code, bs_flag, trade_date, stock_kind;

-- Layer 4: 指令明细 (跳3: fund_aggregation → instruction)
CREATE TABLE instruction_final (
    inst_num       NUMERIC(24,0) NOT NULL,
    inst_date      VARCHAR(8)    NOT NULL,
    fund_code      VARCHAR(8),
    branch_code    VARCHAR(8),
    product_code   VARCHAR(12),
    bs_flag        VARCHAR(2),
    stock_kind     VARCHAR(4),
    trade_qty      NUMERIC(15,2),
    trade_amount   NUMERIC(15,3),
    fee_total      NUMERIC(10,2),
    cost_total     NUMERIC(18,3)
);

-- 跳3: 从聚合表直接映射到指令表
INSERT INTO instruction_final (
    inst_num, inst_date, fund_code, branch_code, product_code,
    bs_flag, stock_kind, trade_qty, trade_amount, fee_total, cost_total
)
SELECT
    70000000001,                                     -- 序列值 (简化)
    trade_date,
    fund_code,
    branch_code,
    product_code,
    bs_flag,
    stock_kind,
    total_qty,                                       -- 1:1 DataFlow
    total_amount,                                    -- 1:1 DataFlow
    total_yhs + total_jsf + total_zgf + total_ghf,   -- 聚合: fee_total = sum of fees
    calc_cost                                        -- 1:1 DataFlow
FROM fund_aggregation;

-- ============================================================
-- 端到端列追踪验证:
-- ============================================================
-- 追踪 instruction_final.trade_qty 的完整链路 (depth=3):
--
-- codeweb lineage instruction_final.trade_qty --direction upstream --depth 3
--
-- 预期输出:
--   instruction_final.trade_qty [DataFlow]
--     └── fund_aggregation.total_qty [Aggregated: SUM, GROUP BY branch_code,product_code,bs_flag,trade_date,stock_kind]
--         └── std_intermediate.trade_qty [Derived: cjsl * trade_unit]
--             └── src_raw.trade_qty [DataFlow]
--
-- 追踪 instruction_final.cost_total 的完整链路 (depth=3):
--
--   instruction_final.cost_total [DataFlow]
--     └── fund_aggregation.calc_cost [Derived: SUM(amount) + sign*SUM(yhs+jsf+zgf+ghf)]
--         ├── std_intermediate.trade_amount [Derived: ABS]
--         │   └── src_raw.trade_amount [DataFlow]
--         ├── std_intermediate.fee_yhs [DataFlow]
--         │   └── src_raw.fee_yhs [DataFlow]
--         ├── std_intermediate.fee_jsf [DataFlow]
--         │   └── src_raw.fee_jsf [DataFlow]
--         ├── std_intermediate.fee_zgf [DataFlow]
--         │   └── src_raw.fee_zgf [DataFlow]
--         └── std_intermediate.fee_ghf [DataFlow]
--             └── src_raw.fee_ghf [DataFlow]
--
-- 关键断言:
--   1. 4跳链路中每跳变换类型不同: DataFlow→Derived→Aggregated→DataFlow
--   2. calc_cost 是 FAN-IN (多列汇聚到一列)
--   3. depth=1 只显示直接上游, depth=3 显示完整链路
