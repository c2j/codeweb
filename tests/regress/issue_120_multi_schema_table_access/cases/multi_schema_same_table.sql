-- Issue #120: Multi-schema same-name TableAccess edges are merged to the
-- first-scanned table due to bare-name alias collision in table_index.
--
-- Two schemas (schema_a, schema_b) each define a table named `tab1` and a
-- stored procedure. schema_a.proc_a uses a qualified reference
-- `schema_a.tab1`; schema_b.proc_b uses a bare reference `tab1`.
--
-- Expected after fix:
--   schema_a.proc_a --TableAccess--> schema_a.tab1  (qualified → correct schema)
--   schema_b.proc_b --TableAccess--> schema_b.tab1  (bare → resolved via owner schema)
--
-- Currently (bug):
--   schema_a.proc_a --TableAccess--> schema_a.tab1  ✓
--   schema_b.proc_b --TableAccess--> schema_a.tab1  ✗
--     (bare name `tab1` hits the first-registered bare alias in table_index,
--      which is schema_a.tab1 because it was scanned first)
--
-- schema_b.tab1 becomes orphaned — it has a CREATE TABLE node but no
-- TableAccess edges from schema_b.proc_b.

CREATE SCHEMA schema_a;
CREATE SCHEMA schema_b;

CREATE TABLE schema_a.tab1 (id INT, val TEXT);
CREATE TABLE schema_b.tab1 (id INT, val TEXT);

-- proc_a: uses schema-qualified table reference → should target schema_a.tab1
CREATE PROCEDURE schema_a.proc_a() AS $$
BEGIN
    INSERT INTO schema_a.tab1 (id, val) VALUES (1, 'a');
END;
$$;

-- proc_b: uses bare (unqualified) table reference → should target schema_b.tab1
--         because proc_b belongs to schema_b
CREATE PROCEDURE schema_b.proc_b() AS $$
BEGIN
    SELECT * FROM tab1;
END;
$$;
