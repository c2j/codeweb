-- ============================================================
-- 测试案例 05: 简单视图 (列直通 + 列重命名)
-- 来源: V_DAT_INST_SECU_ACNT_INFO, V_JK_DAT_INST_INDEX
-- 覆盖: 1:1 视图列映射、列重命名、视图列血缘
-- ============================================================

CREATE TABLE instruction_base (
    inst_num       NUMERIC(24,0) NOT NULL,
    inst_data_date VARCHAR(8)   NOT NULL,
    operation_no   VARCHAR(10),
    fund_code      VARCHAR(8),
    settle_mode    VARCHAR(3),
    data_source    VARCHAR(1)
);

-- ============================================
-- 场景 A: 简单 1:1 视图 (有列重命名)
-- ============================================
-- 基表
CREATE TABLE account_info (
    inst_num       NUMERIC(24,0) NOT NULL,
    out_account    VARCHAR(50),          -- 付款账户
    in_account     VARCHAR(19),          -- 收款账户
    inst_data_date VARCHAR(8)  NOT NULL,
    client_account VARCHAR(19)           -- 客户账户
);

-- 视图: 同名列直通 + 列重命名
CREATE OR REPLACE VIEW v_account_info (
    inst_id,          -- inst_num → inst_id (重命名)
    out_acnt,         -- out_account → out_acnt (重命名)
    in_acnt,          -- in_account → in_acnt (重命名)
    data_date,        -- inst_data_date → data_date (重命名)
    client_acnt       -- client_account → client_acnt (重命名)
) AS
SELECT
    inst_num,
    out_account,
    in_account,
    inst_data_date,
    client_account
FROM account_info t;

-- 预期:
--   codeweb lineage v_account_info.inst_id --direction upstream
--   account_info.inst_num → v_account_info.inst_id [DataFlow, inst_num→inst_id]
--
--   所有列都是 [DataFlow] 类型，但有列重命名标注

-- ============================================
-- 场景 B: 纯 1:1 视图 (无重命名)
-- ============================================
CREATE TABLE index_tags (
    operation_no   VARCHAR(10) NOT NULL,
    inst_num       NUMERIC(24,0) NOT NULL,
    inst_data_date VARCHAR(10) NOT NULL,
    index_tag      VARCHAR(50) NOT NULL,
    index_value    VARCHAR(500) NOT NULL
);

CREATE OR REPLACE VIEW v_index_tags AS
SELECT
    operation_no,
    inst_num,
    inst_data_date,
    index_tag,
    index_value
FROM index_tags;

-- 预期:
--   所有列都是 1:1 DataFlow，无重命名

-- ============================================
-- 场景 C: 视图被后续查询引用
-- ============================================
-- 视图 V_INDEX 的列 inst_num 被 INSERT 引用
INSERT INTO instruction_base (inst_num, inst_data_date, operation_no, settle_mode, data_source)
SELECT
    v.inst_num,
    v.inst_data_date,
    v.operation_no,
    '000',
    'V'
FROM v_index_tags v
WHERE v.index_tag = 'stock_code';

-- 预期 (双层血缘):
--   codeweb lineage instruction_base.inst_num --direction upstream
--   index_tags.inst_num → v_index_tags.inst_num → instruction_base.inst_num [DataFlow]
--   视图作为中间节点出现
