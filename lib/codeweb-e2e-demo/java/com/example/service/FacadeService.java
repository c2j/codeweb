package com.example.service;

import com.example.dao.UserDao;
import com.example.dao.OrderDao;

public class FacadeService {
    private UserService userService;
    private OrderService orderService;
    private UserDao userDao;
    private OrderDao orderDao;

    public void onboarding(String name, String email) {
        userService.createUser(name, email);
        userDao.findById(0L);
    }

    public void offboardUser(Long userId) {
        userService.archiveUser(userId);
        orderService.cancelOrdersForUser(userId);
    }

    public void transferUser(Long userId, Long targetOrg) {
        userDao.transferUser(userId, targetOrg);
        orderDao.cancelByUserId(userId);
    }
}
