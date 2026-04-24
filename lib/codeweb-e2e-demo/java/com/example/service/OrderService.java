package com.example.service;

import com.example.dao.OrderDao;
import com.example.dao.UserDao;
import com.example.util.TextUtil;

public class OrderService {
    private OrderDao orderDao;
    private UserDao userDao;
    private TextUtil textUtil;

    public Object getOrders(Long userId) {
        return orderDao.findByUserId(userId);
    }

    public void cancelOrder(Long orderId) {
        String tag = textUtil.sanitize("CANCEL");
        orderDao.cancelOrder(orderId);
    }

    public void cancelOrdersForUser(Long userId) {
        userDao.deactivateUser(userId);
        orderDao.cancelByUserId(userId);
    }
}
