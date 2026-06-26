-- Realistic complex patterns for EXECUTE IMMEDIATE variable resolution
--
-- These simulate real production PL/SQL patterns where dynamic SQL
-- is built through conditionals, loops, and multi-step construction.
-- Each pattern is a self-contained stored procedure that exercises
-- a distinct code path in the current variable resolution implementation.

CREATE OR REPLACE PROCEDURE shared_callee AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_full_archive   AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_full           AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_incremental    AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_create         AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_update         AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_delete         AS $$ BEGIN NULL; END; $$;
CREATE OR REPLACE PROCEDURE p_batch          AS $$ BEGIN NULL; END; $$;
CREATE TABLE t_config (sql_text TEXT);

-- ── Pattern 1: Nested IF/ELSE building different CALL targets ──
CREATE OR REPLACE PROCEDURE p_nested_if AS $$
DECLARE
    v_mode VARCHAR2(20) := 'FULL';
    v_archive BOOLEAN := TRUE;
    v_sql VARCHAR2(10000);
BEGIN
    IF v_mode = 'FULL' THEN
        IF v_archive THEN
            v_sql := 'CALL p_full_archive()';
        ELSE
            v_sql := 'CALL p_full()';
        END IF;
    ELSE
        v_sql := 'CALL p_incremental()';
    END IF;
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 2: CASE-driven dynamic CALL target ──
CREATE OR REPLACE PROCEDURE p_case_dispatch AS $$
DECLARE
    v_operation VARCHAR2(20) := 'CREATE';
    v_proc VARCHAR2(100);
    v_sql VARCHAR2(10000);
BEGIN
    v_proc := CASE v_operation
        WHEN 'CREATE' THEN 'p_create'
        WHEN 'UPDATE' THEN 'p_update'
        WHEN 'DELETE' THEN 'p_delete'
        ELSE 'p_full'
    END;
    v_sql := 'CALL ' || v_proc || '()';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 3: Incremental WHERE clause building with conditionals ──
CREATE OR REPLACE PROCEDURE p_where_builder AS $$
DECLARE
    v_sql VARCHAR2(10000);
    p_name VARCHAR2(100) := 'test';
    p_status VARCHAR2(20) := NULL;
BEGIN
    v_sql := 'SELECT * FROM t_source WHERE 1=1';
    IF p_name IS NOT NULL THEN
        v_sql := v_sql || ' AND name = ''' || p_name || '''';
    END IF;
    IF p_status IS NOT NULL THEN
        v_sql := v_sql || ' AND status = ''' || p_status || '''';
    END IF;
    v_sql := v_sql || ' ORDER BY id';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 4: FOR loop building SQL from cursor results ──
CREATE OR REPLACE PROCEDURE p_loop_column_list AS $$
DECLARE
    v_sql VARCHAR2(10000);
    v_sep VARCHAR2(10) := '';
BEGIN
    v_sql := 'SELECT ';
    FOR rec IN (SELECT 'col' || generate_series(1, 3) AS c)
    LOOP
        v_sql := v_sql || v_sep || rec.c;
        v_sep := ', ';
    END LOOP;
    v_sql := v_sql || ' FROM t_source';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 5: WHILE loop accumulating CALL arguments ──
CREATE OR REPLACE PROCEDURE p_loop_batch AS $$
DECLARE
    v_sql VARCHAR2(10000);
    v_counter INT := 5;
    v_param VARCHAR2(100) := 'x';
BEGIN
    v_sql := 'CALL p_batch(';
    WHILE v_counter > 0 LOOP
        v_sql := v_sql || '''' || v_param || ''',';
        v_counter := v_counter - 1;
    END LOOP;
    v_sql := v_sql || '''end'')';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 6: Nested IF/CASE double dispatch ──
CREATE OR REPLACE PROCEDURE p_double_dispatch AS $$
DECLARE
    p_action VARCHAR2(20) := 'CREATE';
    p_entity VARCHAR2(20) := 'USER';
    v_proc VARCHAR2(100);
    v_sql VARCHAR2(10000);
BEGIN
    IF p_entity = 'USER' THEN
        v_proc := CASE p_action
            WHEN 'CREATE' THEN 'p_create'
            WHEN 'UPDATE' THEN 'p_update'
            ELSE 'p_full'
        END;
    ELSE
        v_proc := CASE p_action
            WHEN 'CREATE' THEN 'p_create'
            ELSE 'p_full'
        END;
    END IF;
    v_sql := 'CALL ' || v_proc || '()';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 7: FOR loop populating SQL from table config ──
CREATE OR REPLACE PROCEDURE p_config_driven AS $$
DECLARE
    v_sql VARCHAR2(10000);
BEGIN
    FOR cfg IN (SELECT sql_text FROM t_config WHERE ROWNUM = 1)
    LOOP
        v_sql := cfg.sql_text;
    END LOOP;
    IF v_sql IS NOT NULL THEN
        EXECUTE IMMEDIATE v_sql;
    END IF;
END;
$$;

-- ── Pattern 8: Concat chain via DECLARE default (chain resolution) ──
CREATE OR REPLACE PROCEDURE p_declare_chain AS $$
DECLARE
    v_proc VARCHAR2(100) := 'shared_callee';
    v_sql VARCHAR2(10000);
BEGIN
    v_sql := 'CALL ' || v_proc || '()';
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 9: Variable-to-variable chain (v_a := '...'; v_b := v_a) ──
CREATE OR REPLACE PROCEDURE p_var_chain AS $$
DECLARE
    v_src VARCHAR2(100) := 'CALL shared_callee()';
    v_sql VARCHAR2(10000);
BEGIN
    v_sql := v_src;
    EXECUTE IMMEDIATE v_sql;
END;
$$;

-- ── Pattern 10: Incremental WHERE builder (multiple self-ref concat, all vars resolved) ──
CREATE OR REPLACE PROCEDURE p_where_full_resolve AS $$
DECLARE
    v_sql VARCHAR2(10000);
    p_name VARCHAR2(100) := 'test';
BEGIN
    v_sql := 'SELECT * FROM t_source WHERE 1=1';
    IF p_name IS NOT NULL THEN
        v_sql := v_sql || ' AND name = ''' || p_name || '''';
    END IF;
    v_sql := v_sql || ' ORDER BY id';
    EXECUTE IMMEDIATE v_sql;
END;
$$;
