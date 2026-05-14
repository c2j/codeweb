package com.example.service;

import com.example.dao.InventoryDao;
import com.example.dao.OrderDao;
import com.example.dao.PaymentDao;

public class OrderService extends BaseService {

    private OrderDao orderDao;
    private PaymentDao paymentDao;
    private InventoryDao inventoryDao;

    public Object getOrderDetail(Long orderId) {
        return orderDao.findOrderDetail(orderId);
    }

    public void createOrder(Long userId, Long productId, int qty) {
        inventoryDao.checkStock(productId, qty);

        orderDao.createOrder(userId, productId, qty);
        orderDao.findByUserIdA("t_orders", userId);
        logAction("ORDER", "CREATE");
    }

    public void cancelOrder(Long orderId) {
        orderDao.cancelOrder(orderId);
        paymentDao.refundPayment(orderId);
        logAction("ORDER", "CANCEL");
    }

    public void batchCreate(Long userId, String items) {
        orderDao.batchCreateOrders(userId, items);
        logAction("ORDER", "BATCH_CREATE");
    }

    public void completeOrder(Long orderId) {
        orderDao.completeOrder(orderId);
        logAction("ORDER", "COMPLETE");
    }
}
