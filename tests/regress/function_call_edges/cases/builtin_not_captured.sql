-- Regression: built-in functions must NOT be captured as call edges
-- Verifies builtin:None filter in visit_expr — COUNT/SUM/etc are tagged
-- builtin:Some by ogsql-parser and should be skipped.

CREATE TABLE dual (dummy INT);

CREATE PROCEDURE aggregate_data AS $$
BEGIN
    PERFORM COUNT(*) FROM dual;
END;
$$;
