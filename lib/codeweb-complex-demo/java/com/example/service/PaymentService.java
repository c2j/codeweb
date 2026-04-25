package com.example.service;

import com.example.dao.PaymentDao;
import org.apache.ibatis.session.SqlSession;

public class PaymentService extends BaseService {
    private PaymentDao paymentDao;
    private SqlSession sqlSession;

    public void processPayment(Long orderId, double amount, String method) {
        paymentDao.processPayment(orderId, amount, method);
        logAction("PAYMENT", "PROCESS");
    }

    public void refundPayment(Long orderId) {
        paymentDao.refundPayment(orderId);
        logAction("PAYMENT", "REFUND");
    }

    public String queryStatus(Long orderId) {
        return paymentDao.queryStatus(orderId);
    }

    public void reconcileAll(String date) {
        sqlSession.update("com.example.dao.PaymentDao.reconcilePayments", date);
        logAction("PAYMENT", "RECONCILE");
    }
}
