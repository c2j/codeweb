-- ============================================================
-- 测试案例 06: 多表 JOIN 视图 + 列级变换
-- 来源: V_DAT_INST_SECU_DEAL_INFO (3表JOIN + 嵌套DECODE)
-- 覆盖: 视图列跨多表血缘、JOIN 条件列参与变换、嵌套表达式
-- ============================================================

-- 基表 1: 指令明细
CREATE TABLE inst_deal_detail (
    parent_seq_no  NUMERIC(24,0) NOT NULL,
    inst_data_date VARCHAR(8)   NOT NULL,
    seq_no         NUMERIC(24,0),
    product_code   VARCHAR(19)  NOT NULL,
    quantity       NUMERIC(25,8) NOT NULL,
    direction      VARCHAR(1)   NOT NULL,   -- in/out: '0'/'1'
    fee_code       VARCHAR(8)
);

-- 基表 2: 证券主数据
CREATE TABLE secu_master (
    product_code    VARCHAR(19) NOT NULL,
    market_code     VARCHAR(3),
    stock_category  VARCHAR(4),             -- '01'=股票, '02'=国债, '06'=可转债
    begin_date      VARCHAR(8),
    end_date        VARCHAR(8)
);

-- 基表 3: 指令基本信息
CREATE TABLE inst_master (
    inst_num       NUMERIC(24,0) NOT NULL,
    inst_data_date VARCHAR(8)   NOT NULL,
    operation_no   VARCHAR(10)             -- 业务操作编码
);

-- 视图: 3表JOIN + 嵌套DECODE数量变换
CREATE OR REPLACE VIEW v_inst_deal_detail (
    parent_id,                               -- inst_master相关
    data_date,                               -- inst_master相关
    seq_no,
    product_code,
    adj_quantity,                            -- 变换列: 涉及3个表
    direction,
    fee_code
) AS
SELECT
    t.parent_seq_no,                         -- DataFlow: inst_deal_detail
    t.inst_data_date,                        -- DataFlow: inst_deal_detail
    t.seq_no,                                -- DataFlow: inst_deal_detail
    t.product_code,                          -- DataFlow: inst_deal_detail
    -- 核心变换列: 涉及3张表的列
    DECODE(w.operation_no,
           '0601000001', t.quantity / 100,    -- 国债协议回购: 手→张, 使用 inst_master.operation_no
           '0603033001', t.quantity,          -- 可转债: 张, 使用 inst_master.operation_no
           DECODE(s.market_code,              -- 其他: 根据市场代码和证券类别
                  '003', DECODE(s.stock_category, '02', t.quantity / 100, t.quantity),
                  '046', DECODE(s.stock_category, '02', t.quantity / 100, t.quantity),
                  t.quantity)) AS adj_quantity,
    t.direction,                             -- DataFlow: inst_deal_detail
    t.fee_code                               -- DataFlow: inst_deal_detail
FROM inst_deal_detail t
JOIN secu_master s ON t.product_code = s.product_code
    AND t.inst_data_date BETWEEN s.begin_date AND s.end_date
JOIN inst_master w ON t.parent_seq_no = w.inst_num
    AND t.inst_data_date = w.inst_data_date;

-- 预期血缘:
--   codeweb lineage v_inst_deal_detail.adj_quantity --direction upstream
--
--   v_inst_deal_detail.adj_quantity [Derived: nested DECODE(...)]
--     ├── inst_deal_detail.quantity         [参与 DECODE 分子: quantity/100 或 quantity]
--     ├── inst_master.operation_no          [参与 DECODE 外层条件: = '0601000001']
--     ├── secu_master.market_code           [参与 DECODE 内层条件: = '003']
--     └── secu_master.stock_category        [参与 DECODE 内层条件: = '02']
--
--   一个视图列对应 4 个基表列的依赖关系!  关键断言:
--   1. 血缘指向基表列，不是视图列
--   2. 参与条件判断的列 (operation_no, market_code, stock_category)
--      也应出现在血缘中（它们是 "过滤/路由" 角色）
--   3. JOIN 条件列 (product_code, inst_num) 不是 adj_quantity 的血缘源

-- 查询视图后 INSERT 到下游
CREATE TABLE output_deal (
    parent_id    NUMERIC(24,0),
    data_date    VARCHAR(8),
    product_code VARCHAR(19),
    trade_qty    NUMERIC(25,8),
    direction    VARCHAR(1)
);

INSERT INTO output_deal (parent_id, data_date, product_code, trade_qty, direction)
SELECT v.parent_id, v.data_date, v.product_code, v.adj_quantity, v.direction
FROM v_inst_deal_detail v
WHERE v.direction = '0';

-- 预期 (跨视图双层展开):
--   output_deal.trade_qty
--     ← v_inst_deal_detail.adj_quantity + (v_inst_deal_detail 视图定义展开)
--       最终指向: inst_deal_detail.quantity, inst_master.operation_no,
--                secu_master.market_code, secu_master.stock_category
