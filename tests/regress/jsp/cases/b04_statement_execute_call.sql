CREATE OR REPLACE FUNCTION proc_from_stmt() RETURNS VARCHAR AS $$
BEGIN
    RETURN 'called from statement';
END;
$$ LANGUAGE plpgsql;
