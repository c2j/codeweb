<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT * FROM orders WHERE id = ?");
    ps.setLong(1, Long.parseLong(request.getParameter("id")));
    ResultSet rs = ps.executeQuery();
    if (rs.next()) {
      out.println("<p>Order total: " + rs.getDouble("total") + "</p>");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
