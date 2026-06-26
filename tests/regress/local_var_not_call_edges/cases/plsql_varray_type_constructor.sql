-- Regression: PL/SQL block-local TYPE ... IS VARRAY(...) OF ... declarations
-- must be recognised as type names so the constructor call `arr_type(...)`
-- is NOT captured as a function call edge.
--
-- Bug: CallExtractor.visit_pl_declaration used to ignore PlDeclaration::Type,
-- so the type name `arr_type` never entered the `known_types` filter. The
-- initialiser `arr_type('A','B','C','D')` parsed as Expr::FunctionCall and
-- produced a false `unresolved:arr_type` node.
--
-- Distinct from `type_constructor_not_captured.sql`, which only covers
-- top-level `CREATE TYPE` DDL — this case covers DECLARE-block TYPE decls.
--
-- Expected: zero Unresolved nodes (and no false call edges).

CREATE OR REPLACE PROCEDURE PROC_VARRAY AS $$
DECLARE
    TYPE arr_type IS VARRAY(4) OF VARCHAR2(100);
    table_array arr_type := arr_type('V_JK_PAR_FUND_PLAN',
                                     'V_JK_ZYNJ_PAR_FUND_INFO',
                                     'V_JK_BDP_PAR_SYS_AREA',
                                     'V_JK_PAR_SYS_PLAN');
BEGIN
    NULL;
END;
$$;
