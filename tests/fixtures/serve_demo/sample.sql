CREATE OR REPLACE PROCEDURE p_demo_query(
    p_user_id BIGINT
) AS $$
BEGIN
    SELECT id, name, email FROM t_users WHERE id = p_user_id;
    UPDATE t_users SET last_seen = now() WHERE id = p_user_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE p_demo_caller(
    p_user_id BIGINT
) AS $$
BEGIN
    CALL p_demo_query(p_user_id);
    COMMIT;
END;
$$ LANGUAGE plpgsql;
