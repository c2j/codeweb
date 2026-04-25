package com.example.service;

public class NotificationService extends BaseService {
    public void sendOrderNotification(Long orderId, String event) {
        logAction("NOTIFY", "ORDER:" + event);
    }

    public void sendPaymentNotification(Long orderId, String status) {
        logAction("NOTIFY", "PAYMENT:" + status);
    }

    public void broadcastPromotion(Long productId) {
        logAction("NOTIFY", "PROMO:" + productId);
    }
}
