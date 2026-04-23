package com.example.dao;

import java.util.List;

public interface OrderDao {
    List<?> findByUserId(Long userId);
    int cancelOrder(Long orderId);
}
