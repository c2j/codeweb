package com.example.service;

import com.example.dao.UserDao;
import com.example.dao.OrderDao;
import com.example.util.TextUtil;

public class UserService {
    private UserDao userDao;
    private OrderDao orderDao;
    private TextUtil textUtil;

    public Object getUser(Long id) {
        return userDao.findById(id);
    }

    public Object listUsers() {
        return sqlSession.selectList("com.example.dao.UserDao.findAll");
    }

    public void createUser(String name, String email) {
        String formatted = textUtil.trim(name);
        userDao.insertUser(formatted, email);
    }

    public void deactivateUser(Long userId) {
        userDao.deactivateUser(userId);
        orderDao.cancelByUserId(userId);
    }

    public void archiveUser(Long userId) {
        deactivateUser(userId);
        userDao.archiveById(userId);
    }

    private void logAction(String action) {
        System.out.println(action);
    }

    public void batchImport(java.util.List<String> names) {
        for (String name : names) {
            logAction("import:" + name);
        }
    }
}
