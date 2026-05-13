package com.example.repository;

import java.util.List;

/**
 * Repository showing more @Query patterns including text blocks and concatenated strings.
 */
public interface ProductRepository {

    @Query(value = """
        SELECT p.id, p.name, p.category, p.price, p.stock
        FROM t_products p
        WHERE p.status = 'ACTIVE'
        ORDER BY p.name
        """, nativeQuery = true)
    List<Object> findAllActiveProducts();

    @Query(value = """
        SELECT p.*, COALESCE(AVG(r.rating), 0) AS avg_rating
        FROM t_products p
        LEFT JOIN t_reviews r ON r.product_id = p.id
        WHERE p.category = :category
        GROUP BY p.id
        HAVING AVG(r.rating) >= :minRating
        """, nativeQuery = true)
    List<Object> findTopRatedInCategory(String category, double minRating);

    @Query(value = "SELECT id, name, price FROM t_products " +
           "WHERE price BETWEEN :minPrice AND :maxPrice " +
           "AND status = 'ACTIVE' " +
           "ORDER BY price ASC", nativeQuery = true)
    List<Object> findByPriceRange(double minPrice, double maxPrice);
}
