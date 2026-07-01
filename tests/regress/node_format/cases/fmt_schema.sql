CREATE TABLE users (
    id BIGINT PRIMARY KEY,
    name VARCHAR(100),
    email VARCHAR(200),
    status VARCHAR(20)
);

CREATE OR REPLACE PROCEDURE create_user(
    p_id IN BIGINT,
    p_name IN VARCHAR,
    p_email IN VARCHAR
) AS
BEGIN
    INSERT INTO users (id, name, email) VALUES (p_id, p_name, p_email);
END;
/
