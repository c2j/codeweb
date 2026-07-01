<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT id, total FROM orders WHERE created > ?");
    ps.setDate(1, java.sql.Date.valueOf("2024-01-01"));
    rs = ps.executeQuery();
    while (rs.next()) {
      out.println("<tr><td>" + rs.getInt("id") + "</td><td>" + rs.getDouble("total") + "</td></tr>");
    }
  } catch (SQLException e) {
    out.println("<p class='error'>Database error: " + e.getMessage() + "</p>");
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
