-- Regression: TYPE constructor calls and dotted member-method calls must
-- NOT be captured as call edges.
--
-- Three false-positive classes in one body, all parse as
-- Expr::FunctionCall { builtin: None }:
--   1. account_record_table()       — TYPE constructor (DEFAULT initializer)
--   2. obj_account_record.equals(...) — dotted call, first component is a TYPE
--   3. aaa1(i) / aaa2(i)            — collection index on local vars
--
-- Expected: zero Unresolved nodes.

-- ogsql-parser indexes TABLE OF types but not OBJECT types; both TYPEs here
-- use TABLE OF so they enter type_index and reach CallExtractor.known_types.
CREATE TYPE account_record_table AS TABLE OF VARCHAR(100);
CREATE TYPE obj_account_record AS TABLE OF INT;

CREATE PROCEDURE compare_records AS $$
DECLARE
    aaa1 account_record_table := account_record_table();
    aaa2 account_record_table := account_record_table();
    v_idx INTEGER := 1;
    v_result INT;
BEGIN
    FOR v_idx IN 1 .. aaa1.COUNT LOOP
        IF obj_account_record.equals(aaa1(v_idx), aaa2(v_idx)) = 1 THEN
            v_result := 1;
        END IF;
    END LOOP;
END;
$$;
