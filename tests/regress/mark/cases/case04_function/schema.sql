-- Case 04: Function - Direct + Indirect caller-chain
-- Target: function `calc_tax`
-- Graph: calc_total → calc_tax, generate_invoice → calc_total
--    generate_invoice ←DirectCall-- batch_invoice (2-level indirect to calc_tax)

CREATE OR REPLACE FUNCTION calc_tax(amount NUMERIC) RETURNS NUMERIC
AS
BEGIN
    RETURN amount * 0.13;
END;
/

CREATE OR REPLACE FUNCTION calc_total(order_id INT) RETURNS NUMERIC
AS
    v_subtotal NUMERIC;
    v_tax NUMERIC;
BEGIN
    SELECT amount INTO v_subtotal FROM orders WHERE id = order_id;
    v_tax := calc_tax(v_subtotal);
    RETURN v_subtotal + v_tax;
END;
/

CREATE OR REPLACE PROCEDURE generate_invoice(order_id INT)
AS
    v_total NUMERIC;
BEGIN
    v_total := calc_total(order_id);
    INSERT INTO invoices (order_id, total) VALUES (order_id, v_total);
END;
/

CREATE OR REPLACE PROCEDURE batch_invoice()
AS
BEGIN
    CALL generate_invoice(1);
    CALL generate_invoice(2);
END;
/
