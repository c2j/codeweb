package com.example.dao;

public interface PaymentDao {
    int processPayment(Long orderId, double amount, String method);
    int refundPayment(Long orderId);
    String queryStatus(Long orderId);
    int reconcilePayments(String date);
}
