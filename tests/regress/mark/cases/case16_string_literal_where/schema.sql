CREATE TABLE film (
    id INT PRIMARY KEY,
    title VARCHAR(200)
);

CREATE TABLE film_log (
    id INT PRIMARY KEY,
    entity_type VARCHAR(100),
    entity_id INT
);

CREATE OR REPLACE PROCEDURE log_film_access(p_film_id INT)
AS
    v_title VARCHAR(200);
BEGIN
    INSERT INTO film_log (entity_type, entity_id) VALUES ('film', p_film_id);
    SELECT title INTO v_title FROM film WHERE id = p_film_id;
END;
/
