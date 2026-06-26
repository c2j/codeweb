-- Bug reproduction: TYPE name casing mismatch.
--
-- Two CREATE TYPE statements for the same type with different casing.
-- Current bug: builder.rs `type_index` uses raw name / schema.name as keys,
-- so my_test_type and MY_TEST_TYPE produce TWO Type nodes.
--
-- Expected after fix: exactly 1 Type node (merged case-insensitively).
-- A use_type_proc is included so the merged type node has an incident edge (non-orphan).

CREATE OR REPLACE TYPE my_test_type AS (id INT);
/
CREATE OR REPLACE TYPE MY_TEST_TYPE AS (name VARCHAR2(100));
/

CREATE OR REPLACE PROCEDURE use_type_proc IS
    v_rec my_test_type;
BEGIN
    NULL;
END;
/
