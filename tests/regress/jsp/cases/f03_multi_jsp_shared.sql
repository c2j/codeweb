CREATE TABLE customers (
    id BIGINT PRIMARY KEY,
    name VARCHAR(100),
    status VARCHAR(20),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE OR REPLACE FUNCTION get_customer(p_id IN BIGINT) RETURNS VARCHAR AS $$
BEGIN
    RETURN 'customer_' || p_id::TEXT;
END;
$$ LANGUAGE plpgsql;
