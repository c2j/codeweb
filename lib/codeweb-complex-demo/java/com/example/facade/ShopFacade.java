package com.example.facade;

import com.example.service.OrderService;
import com.example.service.PaymentService;
import com.example.service.ProductService;
import com.example.service.InventoryService;
import com.example.service.NotificationService;

public class ShopFacade {
    private OrderService orderService;
    private PaymentService paymentService;
    private ProductService productService;
    private InventoryService inventoryService;
    private NotificationService notificationService;

    public void placeOrder(Long userId, Long productId, int qty) {
        inventoryService.checkStock(productId, qty);
        orderService.createOrder(userId, productId, qty);
        paymentService.processPayment(0L, 0.0, "CREDIT_CARD");
        notificationService.sendOrderNotification(0L, "CREATED");
    }

    public void cancelOrderFull(Long orderId) {
        orderService.cancelOrder(orderId);
        paymentService.refundPayment(orderId);
        notificationService.sendOrderNotification(orderId, "CANCELLED");
    }

    public void updateProductAndNotify(Long productId, double newPrice) {
        productService.updatePrice(productId, newPrice);
        notificationService.broadcastPromotion(productId);
    }

    public void fullInventorySync(Long supplierId) {
        inventoryService.syncFromSupplier(supplierId);
        notificationService.broadcastPromotion(0L);
    }
}
