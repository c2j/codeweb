package com.example.dao;

public interface InventoryDao {
    int checkStock(Long productId, int qty);
    int reserveStock(Long productId, int qty);
    int releaseStock(Long productId, int qty);
    int syncFromSupplier(Long supplierId);
}
