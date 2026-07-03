-- Scenario: trace-sql with partial SQL fragment matches multiple nodes.
-- Tests scoring: exact matches should score higher than substring/partial matches.

CREATE OR REPLACE PROCEDURE proc_get_active_users() AS $$
BEGIN
    SELECT id, name, email
    FROM t_users
    WHERE status = 'ACTIVE'
    ORDER BY name;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_get_users_by_dept(p_dept_id BIGINT) AS $$
BEGIN
    SELECT u.id, u.name, u.email, d.dept_name
    FROM t_users u
    JOIN t_dept d ON u.dept_id = d.id
    WHERE u.dept_id = p_dept_id
      AND u.status = 'ACTIVE'
    ORDER BY u.name;
END;
$$ LANGUAGE plpgsql;
/

CREATE OR REPLACE PROCEDURE proc_deactivate_user(p_user_id BIGINT) AS $$
BEGIN
    UPDATE t_users
    SET status = 'INACTIVE', updated_at = now()
    WHERE id = p_user_id;
END;
$$ LANGUAGE plpgsql;
/
