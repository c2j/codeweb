-- ============================================================
-- 测试案例 09: 跨表列重命名
-- 来源: jsmx_temp.zqzh→out_trd_gh_jy.gddm (多表重命名)
-- 覆盖: 不同表间同一语义字段使用不同列名的映射追踪
-- ============================================================

-- 场景: 三个来源表的同一语义字段 ("股东代码") 在目标表统一为 account_id,
--       但各源表列名各不相同

-- 源表 A: 列名为 zqzh (证券账户)
CREATE TABLE source_a_settlement (
    zqzh          VARCHAR(20),        -- 证券账户 (= 股东代码)
    branch        VARCHAR(8),
    trade_date    VARCHAR(8),
    product       VARCHAR(12),
    bs_flag       VARCHAR(2),
    trade_qty     NUMERIC(15,0),
    trade_amount  NUMERIC(15,2),
    file_source   VARCHAR(1)
);

-- 源表 B: 列名为 gddm (股东代码，中文)
CREATE TABLE source_b_transfer (
    gddm          VARCHAR(20),        -- 股东代码 (= 证券账户)
    gdxm          VARCHAR(50),        -- 股东姓名
    broker        VARCHAR(8),
    trade_date    VARCHAR(8),
    product       VARCHAR(12),
    bs_flag       VARCHAR(2),
    trade_qty     NUMERIC(15,0),
    trade_amount  NUMERIC(15,2),
    file_source   VARCHAR(1)
);

-- 源表 C: 列名为 account (英文)
CREATE TABLE source_c_fixed_income (
    account       VARCHAR(20),        -- 账户 (= 股东代码)
    firm          VARCHAR(8),
    trade_date    VARCHAR(8),
    stock_code    VARCHAR(12),
    dir           VARCHAR(2),
    volume        NUMERIC(15,0),      -- 列名也不同
    amount        NUMERIC(15,2),      -- 列名也不同
    file_source   VARCHAR(1)
);

-- 目标表: 统一列名
CREATE TABLE target_unified (
    account_id    VARCHAR(20),        -- zqzh/gddm/account → account_id (统一)
    account_name  VARCHAR(50),        -- gdxm → account_name
    branch_code   VARCHAR(8),         -- branch/broker/firm → branch_code (统一)
    trade_date    VARCHAR(8),
    product_code  VARCHAR(12),        -- product/stock_code → product_code (统一)
    bs_flag       VARCHAR(2),
    trade_qty     NUMERIC(15,0),      -- trade_qty/volume → trade_qty (统一)
    trade_amount  NUMERIC(15,2),      -- trade_amount/amount → trade_amount (统一)
    file_source   VARCHAR(1)
);

-- UNION ALL INSERT: 三个源表 → 一个目标表
INSERT INTO target_unified (
    account_id, account_name, branch_code, trade_date, product_code,
    bs_flag, trade_qty, trade_amount, file_source
)
-- 分支 A: source_a_settlement
SELECT
    zqzh,                                       -- zqzh → account_id (重命名)
    NULL,                                       -- 无 account_name
    branch,                                     -- branch → branch_code (重命名)
    trade_date,
    product,                                    -- product → product_code (重命名)
    bs_flag,
    trade_qty,
    trade_amount,
    file_source
FROM source_a_settlement
WHERE trade_date = '20250715'

UNION ALL

-- 分支 B: source_b_transfer
SELECT
    gddm,                                       -- gddm → account_id (重命名)
    gdxm,                                       -- gdxm → account_name
    broker,                                     -- broker → branch_code (重命名)
    trade_date,
    product,                                    -- product → product_code (重命名)
    bs_flag,
    trade_qty,
    trade_amount,
    file_source
FROM source_b_transfer
WHERE trade_date = '20250715'

UNION ALL

-- 分支 C: source_c_fixed_income
SELECT
    account,                                    -- account → account_id (重命名)
    NULL,                                       -- 无 account_name
    firm,                                       -- firm → branch_code (重命名)
    trade_date,
    stock_code,                                 -- stock_code → product_code (重命名)
    DECODE(dir, 'B', '0B', 'S', '0S', '0'),    -- dir→bs_flag + DECODE (重命名+变换)
    volume,                                     -- volume → trade_qty (重命名)
    amount,                                     -- amount → trade_amount (重命名)
    file_source
FROM source_c_fixed_income
WHERE trade_date = '20250715';

-- 预期血缘:
--   codeweb lineage target_unified.account_id --direction upstream
--   target_unified.account_id [UNION ALL, 3 sources]
--     ├── source_a_settlement.zqzh     [DataFlow, zqzh→account_id]
--     ├── source_b_transfer.gddm       [DataFlow, gddm→account_id]
--     └── source_c_fixed_income.account [DataFlow, account→account_id]
--
--   codeweb lineage target_unified.bs_flag --direction upstream
--   target_unified.bs_flag [UNION ALL, 3 sources]
--     ├── source_a_settlement.bs_flag  [DataFlow]
--     ├── source_b_transfer.bs_flag    [DataFlow]
--     └── source_c_fixed_income.dir    [Derived: DECODE(dir, 'B','0B','S','0S','0')]
--
-- 关键断言:
--   1. 同一目标列 account_id 有3个不同的重命名源
--   2. source_c 的 bs_flag 是 DECODE 变换 (不是单纯重命名)
--   3. source_a 的 account_name 是 NULL (无来源)
