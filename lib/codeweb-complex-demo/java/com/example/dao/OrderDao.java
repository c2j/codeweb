package com.example.dao;

import java.util.List;

public interface OrderDao {
    List<?> findByUserId(Long userId);
    int createOrder(Long userId, Long productId, int qty);
    int cancelOrder(Long orderId);
    Object findOrderDetail(Long orderId);
    int batchCreateOrders(Long userId, String items);
    int completeOrder(Long orderId);
}
