CREATE TABLE payment (
    id INT PRIMARY KEY,
    amount NUMERIC,
    status VARCHAR(20)
);

CREATE TABLE payment_archive (
    id INT PRIMARY KEY,
    amount NUMERIC,
    status VARCHAR(20)
);

CREATE OR REPLACE PROCEDURE archive_payment(p_id INT)
AS
BEGIN
    INSERT INTO payment_archive (id, amount, status) SELECT id, amount, status FROM payment WHERE id = p_id;
END;
/
