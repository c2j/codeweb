<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    conn.prepareStatement("SELECT * FROM customers");
    conn.prepareStatement("INSERT INTO log VALUES (1, 'test')");
    conn.prepareStatement("UPDATE config SET val = ? WHERE key = ?");
    conn.prepareStatement("DELETE FROM cache WHERE expires < NOW()");
  } finally {
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
