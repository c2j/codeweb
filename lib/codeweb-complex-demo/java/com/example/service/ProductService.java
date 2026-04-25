package com.example.service;

import com.example.dao.ProductDao;
import java.util.List;

public class ProductService extends BaseService {
    private ProductDao productDao;

    public Object getProductInfo(Long productId) {
        return productDao.getProductInfo(productId);
    }

    public List<?> searchProducts(String keyword, String category) {
        return productDao.searchProducts(keyword, category);
    }

    public void updatePrice(Long productId, double newPrice) {
        productDao.updatePrice(productId, newPrice);
        logAction("PRODUCT", "UPDATE_PRICE");
    }

    public void batchUpdatePrices(String category, double multiplier) {
        productDao.batchUpdatePrices(category, multiplier);
        logAction("PRODUCT", "BATCH_UPDATE");
    }

    public void deactivateProduct(Long productId) {
        productDao.deactivateProduct(productId);
        logAction("PRODUCT", "DEACTIVATE");
    }
}
