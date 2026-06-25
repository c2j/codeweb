-- Regression: GaussDB built-in package dbe_xmldom must NOT be captured as
-- Unresolved call nodes. dbe_xmldom is a system package in the ogsql-parser
-- builtin registry (449+ functions); its methods are NOT user-defined routines.
--
-- Bug: statement-level package calls (e.g. dbe_xmldom.setattribute(...)) flow
-- through visit_procedure_call, which bypasses the Expr::FunctionCall builtin
-- field check. The is_known_system_call() fallback whitelist is missing the
-- "DBE_XMLDOM." prefix, so these calls leak through as Node::Unresolved.

CREATE PROCEDURE build_xml_doc AS $$
DECLARE
    l_doc  INTEGER;
    l_elem INTEGER;
BEGIN
    -- expression-context calls (visit_expr path)
    l_doc  := dbe_xmldom.newdomdocument();
    l_elem := dbe_xmldom.createelement(l_doc, 'root');

    -- statement-level calls (visit_procedure_call path) — the reported bug
    dbe_xmldom.setattribute(l_elem, 'id', '1');
    dbe_xmldom.appendchild(l_doc, l_elem);

    -- PERFORM-context call (visit_expr path via PERFORM)
    PERFORM dbe_xmldom.getattribute(l_elem, 'id');
END;
$$;
