package com.example.service;

import org.apache.ibatis.session.SqlSession;

public class ReportService extends BaseService {
    private SqlSession sqlSession;

    public void generateDailyReport(String date) {
        sqlSession.update("com.example.dao.ReportDao.generateDailyReport", date);
        logAction("REPORT", "DAILY");
    }

    public void generateSalesReport(String startDate, String endDate) {
        sqlSession.update("com.example.dao.ReportDao.generateSalesReport", startDate);
        logAction("REPORT", "SALES");
    }

    public void exportReport(Long reportId) {
        sqlSession.update("com.example.dao.ReportDao.exportReport", reportId);
        logAction("REPORT", "EXPORT");
    }

    public void cleanup(int days) {
        sqlSession.update("com.example.dao.ReportDao.cleanupOldReports", days);
        logAction("REPORT", "CLEANUP");
    }
}
