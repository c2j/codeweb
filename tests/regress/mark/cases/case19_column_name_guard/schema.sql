CREATE TABLE orders (
    id INT PRIMARY KEY,
    amount NUMERIC
);

CREATE TABLE order_items (
    id INT PRIMARY KEY,
    order_id INT,
    product_name VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE process_order(p_order_id INT)
AS
BEGIN
    INSERT INTO orders (id, amount) VALUES (p_order_id, 0);
END;
/
