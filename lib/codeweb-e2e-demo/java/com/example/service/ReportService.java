package com.example.service;

import com.example.util.TextUtil;

public class ReportService extends BaseService {
    public void generateReport(Long userId) {
        String safe = TextUtil.sanitize("report");
        exportPdf(safe);
    }
}
