package com.example.dao;

public interface ReportDao {
    int generateDailyReport(String date);
    int generateSalesReport(String startDate, String endDate);
    int exportReport(Long reportId);
    int cleanupOldReports(int days);
}
