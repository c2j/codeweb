CREATE TABLE inventory (
    id INT PRIMARY KEY,
    name VARCHAR(100),
    qty INT
);

CREATE OR REPLACE PROCEDURE check_inventory_status()
AS
    v_qty INT;
BEGIN
    SELECT qty INTO v_qty FROM inventory WHERE id = 1;
END;
/

CREATE OR REPLACE PROCEDURE update_inventory(p_id INT, p_qty INT)
AS
BEGIN
    UPDATE inventory SET qty = p_qty WHERE id = p_id;
END;
/
