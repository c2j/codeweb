package com.example.service;

import com.example.dao.InventoryDao;

public class InventoryService extends BaseService {
    private InventoryDao inventoryDao;

    public int checkStock(Long productId, int qty) {
        return inventoryDao.checkStock(productId, qty);
    }

    public void reserveStock(Long productId, int qty) {
        inventoryDao.reserveStock(productId, qty);
        logAction("INVENTORY", "RESERVE");
    }

    public void releaseStock(Long productId, int qty) {
        inventoryDao.releaseStock(productId, qty);
        logAction("INVENTORY", "RELEASE");
    }

    public void syncFromSupplier(Long supplierId) {
        inventoryDao.syncFromSupplier(supplierId);
        logAction("INVENTORY", "SYNC");
    }
}
