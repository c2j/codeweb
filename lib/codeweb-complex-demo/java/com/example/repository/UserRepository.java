package com.example.repository;

import java.util.List;

/**
 * Spring Data JPA-style repository with @Query annotations.
 * These annotations trigger P0 (Annotation) extraction in ogsql-parser.
 */
public interface UserRepository {

    @Query(value = "SELECT * FROM t_users WHERE status = :status", nativeQuery = true)
    List<Object> findByStatus(String status);

    @Query(value = "SELECT id, name, email, created_at FROM t_users WHERE id = :id", nativeQuery = true)
    Object findById(Long id);

    @Query(value = "SELECT * FROM t_users WHERE name LIKE :keyword ORDER BY created_at DESC", nativeQuery = true)
    List<Object> searchByName(String keyword);

    @Query(value = "SELECT count(*) FROM t_users WHERE status = :status", nativeQuery = true)
    int countByStatus(String status);
}
