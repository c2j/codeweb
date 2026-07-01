<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  Statement stmt = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    String status = request.getParameter("status");
    String sql = "SELECT * FROM orders WHERE status = '" + status + "' AND id = " + request.getParameter("id");
    stmt = conn.createStatement();
    rs = stmt.executeQuery(sql);
    while (rs.next()) {
%>
      <p>Order: <%= rs.getString("order_number") %></p>
<%
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (stmt != null) try { stmt.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
