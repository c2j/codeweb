CREATE OR REPLACE FUNCTION calc_tax(p_amount IN DECIMAL, p_region IN VARCHAR) RETURNS DECIMAL AS $$
DECLARE
    v_rate DECIMAL := 0.0;
BEGIN
    IF p_region = 'NY' THEN
        v_rate := 0.08875;
    ELSE
        v_rate := 0.05;
    END IF;
    RETURN p_amount * v_rate;
END;
$$ LANGUAGE plpgsql;
