package com.example.dao;

/**
 * Class containing SQL string constants.
 * Variable names containing "SQL" with SQL content trigger P2 (Constant) extraction.
 */
public final class QueryConstants {

    private QueryConstants() {}

    // Variable name contains "SQL" → constant extraction by name heuristic
    public static final String SQL_SELECT_ALL_USERS =
        "SELECT id, name, email, status FROM t_users WHERE status = 'ACTIVE'";

    public static final String SQL_COUNT_ACTIVE_ORDERS =
        "SELECT count(*) FROM t_orders WHERE status IN ('CREATED', 'PROCESSING')";

    public static final String SQL_INSERT_AUDIT_LOG =
        "INSERT INTO t_audit_log (action, entity_type, entity_id, created_by, created_at) VALUES (?, ?, ?, ?, NOW())";

    // Content contains SQL keywords → constant extraction by content heuristic
    public static final String FIND_RECENT_PAYMENTS =
        "SELECT * FROM t_payment WHERE created_at >= NOW() - INTERVAL '7 days' ORDER BY created_at DESC";

    // Concatenated SQL constant
    public static final String SQL_PRODUCT_INVENTORY_REPORT =
        "SELECT p.id, p.name, p.category, p.price, i.qty_available, i.warehouse_location "
        + "FROM t_products p "
        + "LEFT JOIN t_inventory i ON i.product_id = p.id "
        + "WHERE p.status = 'ACTIVE' "
        + "ORDER BY p.category, p.name";
}
