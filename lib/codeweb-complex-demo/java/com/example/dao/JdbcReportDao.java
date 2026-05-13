package com.example.dao;

import java.sql.Connection;
import java.sql.PreparedStatement;
import java.sql.ResultSet;
import java.sql.SQLException;

/**
 * JDBC-style DAO that uses prepareStatement and executeQuery with embedded SQL.
 * These method calls trigger P1 (MethodCall) extraction in ogsql-parser.
 */
public class JdbcReportDao {

    private Connection conn;

    public JdbcReportDao(Connection conn) {
        this.conn = conn;
    }

    public ResultSet findReportsByDate(String date) throws SQLException {
        PreparedStatement ps = conn.prepareStatement(
            "SELECT id, title, content, created_at FROM t_reports WHERE created_at::date = ? ORDER BY created_at DESC"
        );
        ps.setString(1, date);
        return ps.executeQuery();
    }

    public ResultSet getMonthlySummary(int year, int month) throws SQLException {
        PreparedStatement ps = conn.prepareStatement(
            "CALL pkg_report.get_monthly_summary(?, ?)"
        );
        ps.setInt(1, year);
        ps.setInt(2, month);
        return ps.executeQuery();
    }

    public int deleteOldReports(int days) throws SQLException {
        PreparedStatement ps = conn.prepareStatement(
            "DELETE FROM t_reports WHERE created_at < NOW() - INTERVAL '1 day' * ?"
        );
        ps.setInt(1, days);
        return ps.executeUpdate();
    }

    public ResultSet getSalesSummary(String startDate, String endDate) throws SQLException {
        PreparedStatement ps = conn.prepareStatement(
            "SELECT p.category, SUM(o.total_amount) AS total_sales " +
            "FROM t_orders o JOIN t_products p ON o.product_id = p.id " +
            "WHERE o.created_at >= ?::date AND o.created_at < ?::date + 1 " +
            "AND o.status = 'COMPLETED' " +
            "GROUP BY p.category ORDER BY total_sales DESC"
        );
        ps.setString(1, startDate);
        ps.setString(2, endDate);
        return ps.executeQuery();
    }
}
