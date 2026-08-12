-- ============================================================
-- 测试案例 11: 匿名 PL/SQL 块中的 INSERT SELECT
-- 覆盖: 匿名块 + 存储过程中的 INSERT SELECT 列级血缘
-- ============================================================

-- DDL
CREATE TABLE daily_input (
    account_id   VARCHAR(20),
    trade_date   VARCHAR(8),
    product_code VARCHAR(12),
    trade_qty    NUMERIC(15,0),
    trade_amt    NUMERIC(15,2)
);

CREATE TABLE daily_output (
    account_id   VARCHAR(20),
    trade_date   VARCHAR(8),
    product_code VARCHAR(12),
    total_qty    NUMERIC(15,0),
    total_amt    NUMERIC(15,2),
    process_date VARCHAR(8)
);

-- ============================================
-- 匿名块: 含变量、循环、INSERT SELECT
-- ============================================
DECLARE
    v_date       VARCHAR(8) := '20250715';
    v_row_count  NUMBER := 0;
    v_total_qty  NUMERIC(15,0);
    v_total_amt  NUMERIC(15,2);

    -- 游标定义
    CURSOR c_input IS
        SELECT account_id, trade_date, product_code,
               SUM(trade_qty) sum_qty,
               SUM(trade_amt) sum_amt
          FROM daily_input
         WHERE trade_date = v_date
         GROUP BY account_id, trade_date, product_code;
    r_input c_input%ROWTYPE;
BEGIN
    -- 清理旧数据
    DELETE FROM daily_output WHERE trade_date = v_date;
    COMMIT;

    -- 游标循环处理
    OPEN c_input;
    LOOP
        FETCH c_input INTO r_input;
        EXIT WHEN c_input%NOTFOUND;

        -- 插入聚合结果
        INSERT INTO daily_output (
            account_id, trade_date, product_code,
            total_qty, total_amt, process_date
        ) VALUES (
            r_input.account_id,
            r_input.trade_date,
            r_input.product_code,
            r_input.sum_qty,               -- 聚合列: SUM(trade_qty)
            r_input.sum_amt,               -- 聚合列: SUM(trade_amt)
            v_date                         -- 变量值
        );

        v_row_count := v_row_count + 1;
        v_total_qty := v_total_qty + NVL(r_input.sum_qty, 0);
        v_total_amt := v_total_amt + NVL(r_input.sum_amt, 0);
    END LOOP;
    CLOSE c_input;

    COMMIT;
END;
/

-- 预期血缘:
--   codeweb lineage daily_output.total_qty --direction upstream
--   daily_input.trade_qty → (SUM聚合) → c_input.sum_qty → daily_output.total_qty [Aggregated: SUM]
--
--   注意: 匿名块中的游标聚合也需要被追踪

-- ============================================
-- 存储过程: INSERT SELECT with 变量拼接
-- ============================================
CREATE TABLE proc_staging (
    fund_code     VARCHAR(8),
    branch_code   VARCHAR(8),
    trade_date    VARCHAR(8),
    total_qty     NUMERIC(15,0),
    total_amount  NUMERIC(15,3),
    commission    NUMERIC(20,8),
    market_code   VARCHAR(3)
);

CREATE TABLE proc_main (
    fund_code     VARCHAR(8),
    branch_code   VARCHAR(8),
    product_code  VARCHAR(12),
    bs_flag       VARCHAR(2),
    trade_date    VARCHAR(8),
    trade_qty     NUMERIC(15,2),
    trade_amount  NUMERIC(15,3),
    fee_yhs       NUMERIC(10,2),
    fee_jsf       NUMERIC(10,2),
    fee_zgf       NUMERIC(10,2),
    fee_ghf       NUMERIC(10,2)
);

CREATE OR REPLACE PROCEDURE proc_aggregate_by_fund(
    p_date    VARCHAR2,
    p_market  VARCHAR2
) IS
    v_proc_name VARCHAR2(50) := 'proc_aggregate_by_fund';
    v_row       NUMBER := 0;
BEGIN
    -- 删除旧数据
    DELETE FROM proc_staging WHERE trade_date = p_date AND market_code = p_market;

    -- INSERT SELECT with aggregation
    INSERT INTO proc_staging (
        fund_code, branch_code, trade_date,
        total_qty, total_amount, commission, market_code
    )
    SELECT
        'F' || SUBSTR(t.account_id, 1, 6),          -- 表达式: 字符串拼接
        t.branch_code,
        t.trade_date,
        SUM(t.trade_qty),                            -- 聚合
        SUM(t.trade_amount),                         -- 聚合
        SUM(t.fee_yhs) + SUM(t.fee_jsf) +           -- 聚合: 费用合计
        SUM(t.fee_zgf) + SUM(t.fee_ghf),
        p_market                                      -- 参数值
    FROM proc_main t
    WHERE t.trade_date = p_date
    GROUP BY SUBSTR(t.account_id, 1, 6), t.branch_code, t.trade_date;

    v_row := SQL%ROWCOUNT;
    COMMIT;
END;
/

-- 调用存储过程
CALL proc_aggregate_by_fund('20250715', '001');

-- 预期血缘:
--   codeweb lineage proc_staging.fund_code --direction upstream
--   proc_main.account_id → proc_staging.fund_code [Derived: 'F' || SUBSTR(account_id,1,6)]
--
--   codeweb lineage proc_staging.commission --direction upstream
--   proc_staging.commission [Derived: SUM(yhs)+SUM(jsf)+SUM(zgf)+SUM(ghf)]
--     ├── proc_main.fee_yhs [Aggregated: SUM]
--     ├── proc_main.fee_jsf [Aggregated: SUM]
--     ├── proc_main.fee_zgf [Aggregated: SUM]
--     └── proc_main.fee_ghf [Aggregated: SUM]
