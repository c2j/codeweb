-- Regression for #140: table references inside subqueries must produce read
-- edges regardless of the outer statement kind.
--
-- Bug: visit_select / visit_insert returned SkipChildren without walking the
-- expression-bearing fields, so subqueries in SELECT/INSERT contexts were
-- silently dropped (degree-0 orphan tables). Only UPDATE/DELETE contexts
-- descended into WHERE expressions and captured them.

CREATE TABLE t_main (id NUMBER, flag VARCHAR2(1));
CREATE TABLE t_parent (id NUMBER);
CREATE TABLE t_audit (id NUMBER);
CREATE TABLE t_excl (id NUMBER);
CREATE TABLE t_out (id NUMBER);
CREATE TABLE t_scalar (id NUMBER, x NUMBER);
CREATE TABLE t_cursor (id NUMBER);

CREATE OR REPLACE PROCEDURE prc_repro AS
  v_cnt NUMBER;
  CURSOR cur IS
    SELECT c.id FROM t_cursor c
     WHERE c.id IN (SELECT p.id FROM t_parent p);
BEGIN
  -- Case 1: SELECT INTO with IN subquery -> t_parent read
  SELECT COUNT(1) INTO v_cnt FROM t_main m
   WHERE m.id IN (SELECT p.id FROM t_parent p);

  -- Case 2: UPDATE with EXISTS subquery -> t_audit read
  UPDATE t_main m SET m.flag = '1'
   WHERE EXISTS (SELECT 1 FROM t_audit a WHERE a.id = m.id);

  -- Case 3: INSERT..SELECT with NOT EXISTS subquery -> t_excl read
  INSERT INTO t_out SELECT m.id FROM t_main m
   WHERE NOT EXISTS (SELECT 1 FROM t_excl e WHERE e.id = m.id);

  -- Case 4: scalar subquery in SELECT list -> t_scalar read
  SELECT m.id, (SELECT MAX(x) FROM t_scalar s) INTO v_cnt, v_cnt FROM t_main m;

  -- Case 5: cursor declaration with IN subquery -> t_cursor / t_parent read
  FOR r IN cur LOOP
    NULL;
  END LOOP;

  -- Case 6: CTE referenced from inside a subquery must NOT become an edge
  WITH cte_sub AS (SELECT id FROM t_parent)
  SELECT COUNT(1) INTO v_cnt FROM t_main m
   WHERE m.id IN (SELECT id FROM cte_sub);
END;
/
