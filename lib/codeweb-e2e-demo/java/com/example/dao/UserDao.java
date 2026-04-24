package com.example.dao;

import java.util.List;

public interface UserDao {
    List<?> findById(Long id);
    List<?> findAll();
    int insertUser(String name, String email);
    int deactivateUser(Long userId);
    int archiveById(Long userId);
    int transferUser(Long userId, Long targetOrg);
}
