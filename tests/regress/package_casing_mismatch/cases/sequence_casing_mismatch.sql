-- Bug reproduction: SEQUENCE name casing mismatch.
--
-- Two CREATE SEQUENCE statements for the same sequence with different casing.
-- Current bug: builder.rs `sequence_index` uses raw name / schema.name as keys,
-- so my_test_seq and MY_TEST_SEQ produce TWO Sequence nodes.
--
-- Expected after fix: exactly 1 Sequence node (merged case-insensitively).
-- A use_seq_proc is included so the merged sequence node has an incident edge (non-orphan).

CREATE SEQUENCE my_test_seq START 1;
/
CREATE SEQUENCE MY_TEST_SEQ START 100;
/

CREATE OR REPLACE PROCEDURE use_seq_proc IS
    v_val INT;
BEGIN
    v_val := my_test_seq.NEXTVAL;
END;
/
