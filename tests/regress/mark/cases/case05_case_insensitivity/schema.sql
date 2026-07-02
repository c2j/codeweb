-- Case 05: Case Insensitivity
-- Same graph as case01 but CSV uses different casing
-- Target: table `Orders` (mixed case in CSV)
-- Graph: Orders ←TableAccess-- Update_Orders

CREATE TABLE Orders (
    id INT PRIMARY KEY,
    amount NUMERIC(10, 2),
    status VARCHAR(20)
);

CREATE OR REPLACE PROCEDURE Update_Orders()
AS
BEGIN
    UPDATE Orders SET status = 'DONE';
END;
/
