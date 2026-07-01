package com.example;

import java.sql.*;

public class UserDao {
    private static final String FIND_SQL = "SELECT id, name FROM users WHERE id = ?";

    public String findUser(int id) {
        Connection conn = DriverManager.getConnection("jdbc:default:connection");
        PreparedStatement ps = conn.prepareStatement(FIND_SQL);
        ps.setInt(1, id);
        ResultSet rs = ps.executeQuery();
        if (rs.next()) return rs.getString("name");
        return null;
    }
}
