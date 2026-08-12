-- ============================================================
-- 测试案例 04: 算术表达式变换 (Derived)
-- 来源: out_trd_gh_jy UPDATE cjsl*cjsl*trade_unit
--        out_trd_bjgsyh INSERT cjje = vol*10*price + vol*10*fee
-- 覆盖: 乘除运算、多列算术组合、整数/日期运算
-- ============================================================

-- ============================================
-- 场景 A: 单列乘法 (cjsl = cjsl * trade_unit)
-- ============================================
CREATE TABLE raw_bond_trade (
    account_id   VARCHAR(20),
    product_code VARCHAR(12),
    trade_qty    NUMERIC(15,2),
    trade_amount NUMERIC(15,3),
    trade_unit   NUMERIC(5,0) DEFAULT 1,
    trade_date   VARCHAR(8)
);

-- 插入原始数据（trade_unit = 1 的交易）
INSERT INTO raw_bond_trade VALUES
    ('A001', '010001', 1000, 100000.00, 1, '20250715'),
    ('A001', '020002', 500,   50000.00, 10, '20250715'),
    ('A002', '030003', 200,   20000.00, 100, '20250715');

-- UPDATE: 对 trade_unit > 1 的数据做数量标准化
UPDATE raw_bond_trade t
   SET t.trade_qty = t.trade_qty * t.trade_unit      -- 算术表达式
 WHERE t.trade_amount >= 0
   AND t.trade_unit > 1;

-- 预期:
--   raw_bond_trade.trade_qty (原值) + raw_bond_trade.trade_unit
--     → raw_bond_trade.trade_qty (新值) [Derived: cjsl * trade_unit]
-- 注意: 这是自引用更新 —— 同表内列级变换

-- ============================================
-- 场景 B: 多列算术组合 (cjje = vol*10*price + vol*10*fee)
-- ============================================
CREATE TABLE bond_deal_raw (
    trade_no     VARCHAR(10),
    trade_date   VARCHAR(8),
    account      VARCHAR(20),
    product_code VARCHAR(8),
    vol          NUMERIC(15,0),
    net_price    NUMERIC(10,3),
    full_price   NUMERIC(10,3),
    face_value   NUMERIC(10,0),
    net_sum      NUMERIC(12,2),
    interest     NUMERIC(10,4)
);

CREATE TABLE bond_deal_output (
    trade_no     VARCHAR(10),
    trade_date   VARCHAR(8),
    account      VARCHAR(20),
    product_code VARCHAR(8),
    bs_flag      VARCHAR(2),
    trade_qty    NUMERIC(15,0),
    trade_amount NUMERIC(16,2),     -- 复合计算: qty*price + qty*fee
    full_amount  NUMERIC(12,2),
    net_sum      NUMERIC(12,2),
    clean_price  NUMERIC(12,2)       -- 复合计算: (amount - interest) / qty
);

INSERT INTO bond_deal_output (
    trade_no, trade_date, account, product_code, bs_flag,
    trade_qty, trade_amount, full_amount, net_sum, clean_price
)
SELECT
    a.trade_no,
    a.trade_date,
    a.account,
    a.product_code,
    '0B',                                            -- 硬编码常量
    a.vol * 10,                                       -- 数量换算: 手→张
    a.vol * 10 * a.net_price +
        a.vol * 10 * 0.001,                           -- 复合计算: 净价金额 + 佣金
    a.full_price * a.vol * 10,                        -- 全价金额
    a.net_sum,                                        -- 直接映射
    (a.vol * 10 * a.net_price - a.interest * 100) /
        (a.vol * 10)                                  -- 净价反算
FROM bond_deal_raw a
WHERE a.trade_date = '20250715';

-- 预期:
--   trade_amount:
--     bond_deal_raw.vol       [参与 vol*10*net_price + vol*10*0.001]
--     bond_deal_raw.net_price [参与 vol*10*net_price + vol*10*0.001]
--     → bond_deal_output.trade_amount [Derived: vol*10*price + vol*10*0.001]
--
--   trade_qty:
--     bond_deal_raw.vol
--     → bond_deal_output.trade_qty [Derived: vol * 10]

-- ============================================
-- 场景 C: NVL/COALESCE (空值处理)
-- ============================================
CREATE TABLE order_raw (
    trade_seq  VARCHAR(16),
    order_qty  NUMERIC(15,0),
    order_amt  NUMERIC(15,2)
);

CREATE TABLE order_processed (
    trade_seq  VARCHAR(16),
    calc_qty   NUMERIC(15,0),
    calc_amt   NUMERIC(15,2)
);

INSERT INTO order_processed (trade_seq, calc_qty, calc_amt)
SELECT
    trade_seq,
    NVL(order_qty, 0),
    NVL(order_qty, 0) * NVL(order_amt, 0)
FROM order_raw;

-- 预期:
--   calc_amt:
--     order_raw.order_qty + order_raw.order_amt
--     → order_processed.calc_amt [Derived: NVL(qty,0) * NVL(amt,0)]
