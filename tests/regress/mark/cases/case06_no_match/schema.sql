-- Case 06: No Match / Empty Result
-- Target: table `nonexistent_table` - not in graph
-- But we still create some unrelated nodes to ensure the graph is non-empty

CREATE TABLE products (
    id INT PRIMARY KEY,
    name VARCHAR(100)
);

CREATE OR REPLACE PROCEDURE list_products()
AS
BEGIN
    SELECT * FROM products;
END;
/
