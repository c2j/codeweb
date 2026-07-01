<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement("UPDATE products SET price = ? WHERE id = ?");
    ps.setBigDecimal(1, new java.math.BigDecimal(request.getParameter("price")));
    ps.setInt(2, Integer.parseInt(request.getParameter("id")));
    int rows = ps.executeUpdate();
    if (rows > 0) {
      out.println("Product price updated.");
    } else {
      out.println("Product not found.");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
