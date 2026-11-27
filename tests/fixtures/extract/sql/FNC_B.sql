-- FNC_B.sql — issue #193 example: b1 (needed) and b2 (unrelated) in one file.
-- b1 writes tbl_c1, extending the closure chain a1 → b1 → tbl_c1 across files.
CREATE FUNCTION b1(p INT) RETURNS INTEGER AS $$
BEGIN
    INSERT INTO tbl_c1 VALUES (p);
    RETURN p;
END;
$$;

CREATE FUNCTION b2() RETURNS INTEGER AS $$
BEGIN
    RETURN 2;
END;
$$;
