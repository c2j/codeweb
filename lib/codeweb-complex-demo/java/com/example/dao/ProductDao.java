package com.example.dao;

import java.util.List;

public interface ProductDao {
    Object getProductInfo(Long productId);
    List<?> searchProducts(String keyword, String category);
    int updatePrice(Long productId, double newPrice);
    int batchUpdatePrices(String category, double multiplier);
    int deactivateProduct(Long productId);
}
