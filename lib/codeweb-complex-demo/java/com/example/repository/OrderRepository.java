package com.example.repository;

import java.util.List;

/**
 * Repository with both @Query and @SqlUpdate annotations.
 * Demonstrates annotation-based SQL extraction with DML statements.
 */
public interface OrderRepository {

    @Query(value = "SELECT * FROM t_orders WHERE user_id = :userId ORDER BY created_at DESC", nativeQuery = true)
    List<Object> findOrdersByUser(Long userId);

    @Query(value = "SELECT o.*, p.name AS product_name FROM t_orders o JOIN t_products p ON o.product_id = p.id WHERE o.id = :orderId", nativeQuery = true)
    Object findOrderWithProduct(Long orderId);

    @Query(value = "SELECT sum(total_amount) FROM t_orders WHERE status = 'COMPLETED' AND created_at >= :since", nativeQuery = true)
    double getTotalRevenue(String since);

    @SqlUpdate("INSERT INTO t_orders (user_id, product_id, qty, total_amount, status) VALUES (:userId, :productId, :qty, :total, 'CREATED')")
    int insertOrder(Long userId, Long productId, int qty, double total);

    @SqlUpdate("UPDATE t_orders SET status = 'CANCELLED' WHERE id = :orderId")
    int markOrderCancelled(Long orderId);

    @SqlUpdate("UPDATE t_orders SET status = 'COMPLETED' WHERE id = :orderId")
    int markOrderCompleted(Long orderId);
}
