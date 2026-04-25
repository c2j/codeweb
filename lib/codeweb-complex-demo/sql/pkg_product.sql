CREATE OR REPLACE PROCEDURE pkg_product.get_product_info(
    p_product_id BIGINT
) AS $$
BEGIN
    SELECT id, name, price, stock_qty FROM t_products WHERE id = p_product_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_product.search_products(
    p_keyword VARCHAR,
    p_category VARCHAR
) AS $$
BEGIN
    SELECT * FROM t_products
    WHERE name LIKE '%' || p_keyword || '%'
      AND (p_category IS NULL OR category = p_category);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_product.update_product_price(
    p_product_id BIGINT,
    p_new_price NUMERIC
) AS $$
DECLARE
    v_formatted VARCHAR;
BEGIN
    v_formatted := pkg_common.format_amount(p_new_price);
    UPDATE t_products SET price = p_new_price WHERE id = p_product_id;
    PERFORM pkg_common.log_operation('PRODUCT', 'UPDATE_PRICE', p_product_id);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_product.batch_update_prices(
    p_category VARCHAR,
    p_multiplier NUMERIC
) AS $$
BEGIN
    UPDATE t_products SET price = price * p_multiplier WHERE category = p_category;
    PERFORM pkg_common.log_operation('PRODUCT', 'BATCH_PRICE', 0);
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE PROCEDURE pkg_product.deactivate_product(
    p_product_id BIGINT
) AS $$
BEGIN
    UPDATE t_products SET active = false WHERE id = p_product_id;
    PERFORM pkg_common.log_operation('PRODUCT', 'DEACTIVATE', p_product_id);
    PERFORM pkg_common.send_notification('EMAIL', 'Product deactivated');
END;
$$ LANGUAGE plpgsql;
