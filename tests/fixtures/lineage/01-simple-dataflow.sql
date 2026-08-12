-- ============================================================
-- 测试案例 01: 简单 INSERT SELECT 直通 (DataFlow)
-- 来源: gh_temp → out_trd_gh_zyk (PKG_SPLIT_TRADE_STEP2)
-- 覆盖: 列级 1:1 直接映射 + 硬编码常量 + 列重命名
-- ============================================================

-- 源表（输入）
CREATE TABLE trade_raw (
    account_id   VARCHAR(20) NOT NULL,    -- 股东代码
    account_name VARCHAR(50),             -- 股东姓名
    branch_code  VARCHAR(8),              -- 席位代码
    product_code VARCHAR(12),             -- 证券代码
    trade_seq    NUMERIC(16,0) NOT NULL,  -- 成交编号
    order_seq    VARCHAR(16),             -- 申报编号
    dealer_code  VARCHAR(8),              -- 交易单元
    bs_flag      VARCHAR(2),              -- 买卖标志
    trade_qty    NUMERIC(18,3),           -- 成交数量
    trade_price  NUMERIC(17,5),           -- 成交价格
    trade_amount NUMERIC(19,5),           -- 成交金额
    remain_qty   NUMERIC(17,2),           -- 持仓余额
    submit_time  VARCHAR(6),              -- 申报时间
    trade_time   VARCHAR(6),              -- 成交时间
    trade_date   VARCHAR(8) NOT NULL,     -- 成交日期
    data_source  VARCHAR(1)               -- 数据来源
);

-- 目标表（标准化后）
CREATE TABLE trade_normalized (
    account_id       VARCHAR(20) NOT NULL,  -- trade_raw.account_id → (同名映射)
    account_name     VARCHAR(50),           -- trade_raw.account_name → (同名映射)
    branch_code      VARCHAR(8),            -- trade_raw.branch_code → (同名映射)
    product_code     VARCHAR(12),           -- trade_raw.product_code → (同名映射)
    trade_seq        NUMERIC(16,0),
    order_seq        VARCHAR(16),
    dealer_code      VARCHAR(8),
    bs_flag          VARCHAR(2),
    trade_qty        NUMERIC(18,3),
    trade_price      NUMERIC(17,5),
    trade_amount     NUMERIC(19,5),
    remain_qty       NUMERIC(17,2),
    submit_time      VARCHAR(6),
    trade_time       VARCHAR(6),
    trade_date       VARCHAR(8),
    sub_partner_code VARCHAR(12),           -- 硬编码为 '000000'
    check_type       VARCHAR(4),            -- 硬编码为 '0'
    data_source      VARCHAR(1)
);

-- INSERT SELECT: 大部分列直通，两列为硬编码常量
INSERT INTO trade_normalized (
    account_id, account_name, branch_code, product_code,
    trade_seq, order_seq, dealer_code, bs_flag,
    trade_qty, trade_price, trade_amount, remain_qty,
    submit_time, trade_time, trade_date,
    sub_partner_code, check_type, data_source
)
SELECT
    t.account_id,
    t.account_name,
    t.branch_code,
    t.product_code,
    t.trade_seq,
    t.order_seq,
    t.dealer_code,
    t.bs_flag,
    t.trade_qty,
    t.trade_price,
    t.trade_amount,
    t.remain_qty,
    t.submit_time,
    t.trade_time,
    t.trade_date,
    '000000',          -- 硬编码常量 → sub_partner_code
    '0',               -- 硬编码常量 → check_type
    t.data_source
FROM trade_raw t
WHERE t.trade_date = '20250715'
  AND t.product_code IS NOT NULL
  AND t.bs_flag IS NOT NULL;

-- 预期血缘输出:
--   codeweb lineage trade_normalized.account_id --direction upstream
--   trade_raw.account_id → trade_normalized.account_id [DataFlow]
--
--   codeweb lineage trade_normalized.sub_partner_code --direction upstream
--   (constant '000000') → trade_normalized.sub_partner_code [Literal]
--
--   codeweb lineage trade_normalized --direction upstream (表级)
--   trade_raw [table] → trade_normalized [W: INSERT INTO ... SELECT FROM]
