<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  Statement stmt = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    stmt = conn.createStatement();
    ResultSet rs = stmt.executeQuery("SELECT COUNT(*) AS cnt FROM items WHERE active = 1");
    if (rs.next()) {
      out.println("<p>Active items: " + rs.getInt("cnt") + "</p>");
    }
  } finally {
    if (stmt != null) try { stmt.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
