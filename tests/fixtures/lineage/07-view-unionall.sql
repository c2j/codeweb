-- ============================================================
-- 测试案例 07: UNION ALL 视图 (多源归并)
-- 来源: V_PAR_BOND (8路UNION ALL)
-- 覆盖: 多基表映射到同一视图列、列语义重映射
-- ============================================================

-- 基表 1: 债券主数据
CREATE TABLE bond_master (
    bond_code      VARCHAR(12) NOT NULL,
    bond_name      VARCHAR(60),
    market_code    VARCHAR(3),
    pay_type       VARCHAR(2),
    bond_category  VARCHAR(4)
);

-- 基表 2: 委托贷款
CREATE TABLE entrust_loan (
    entrust_code   VARCHAR(12) NOT NULL,
    entrust_name   VARCHAR(60),
    market_code    VARCHAR(3),
    loan_type      VARCHAR(2),
    load_amount    NUMERIC(15,2),       -- 注意: 列名不同
    entrust_category VARCHAR(4)
);

-- 基表 3: 权益凭证
CREATE TABLE bond_right (
    right_code     VARCHAR(12) NOT NULL,
    right_name     VARCHAR(60),
    market_code    VARCHAR(3),
    right_type     VARCHAR(2),
    right_category VARCHAR(4)
);

-- UNION ALL 视图: 将3类资产统一为 "bond" 接口
--   关键: 各基表列名不同, 通过 SELECT AS 统一
CREATE OR REPLACE VIEW v_unified_bond (
    product_code,        -- 不同源: bond_code / entrust_code / right_code
    product_name,        -- 不同源: bond_name / entrust_name / right_name
    market_code,
    bond_kind,           -- 不同源: pay_type / loan_type / right_type
    bond_category
) AS
SELECT
    bond_code,
    bond_name,
    market_code,
    pay_type,
    bond_category
FROM bond_master
UNION ALL
SELECT
    entrust_code,                     -- entrust_code → product_code (列重命名)
    entrust_name,                     -- entrust_name → product_name (列重命名)
    market_code,
    loan_type,                        -- loan_type → bond_kind (列重命名+语义重映射)
    entrust_category
FROM entrust_loan
UNION ALL
SELECT
    right_code,                       -- right_code → product_code (列重命名)
    right_name,                       -- right_name → product_name (列重命名)
    market_code,
    right_type,                       -- right_type → bond_kind (列重命名+语义重映射)
    right_category
FROM bond_right;

-- 预期血缘:
--   codeweb lineage v_unified_bond.product_code --direction upstream
--   v_unified_bond.product_code [UNION ALL, 3 sources]
--     ├── bond_master.bond_code       [DataFlow, bond_code→product_code]
--     ├── entrust_loan.entrust_code   [DataFlow, entrust_code→product_code]
--     └── bond_right.right_code       [DataFlow, right_code→product_code]
--
--   codeweb lineage v_unified_bond.bond_kind --direction upstream
--   v_unified_bond.bond_kind [UNION ALL, 3 sources]
--     ├── bond_master.pay_type        [DataFlow, pay_type→bond_kind]
--     ├── entrust_loan.loan_type      [DataFlow, loan_type→bond_kind]
--     └── bond_right.right_type       [DataFlow, right_type→bond_kind]
--
-- 关键断言:
--   1. 每个 UNION ALL 分支输出完整的列级血缘
--   2. 列重命名被正确追踪

-- 视图被下游引用
CREATE TABLE bond_summary (
    product_code VARCHAR(12),
    product_name VARCHAR(60),
    bond_kind    VARCHAR(2)
);

INSERT INTO bond_summary (product_code, product_name, bond_kind)
SELECT product_code, product_name, bond_kind
FROM v_unified_bond
WHERE market_code = '001';

-- 预期 (展开视图):
--   bond_summary.product_code
--     ← v_unified_bond.product_code ← bond_master.bond_code / entrust_loan.entrust_code / bond_right.right_code
