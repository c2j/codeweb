CREATE TABLE orders (
    id BIGINT PRIMARY KEY,
    customer_id BIGINT,
    total DECIMAL(12,2),
    status VARCHAR(20) DEFAULT 'pending',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
