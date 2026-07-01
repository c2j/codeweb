<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT name, price FROM products WHERE id = ?");
    ps.setLong(1, Long.parseLong(request.getParameter("id")));
    rs = ps.executeQuery();
    if (rs.next()) {
      out.println("<h2>" + rs.getString("name") + "</h2>");
      out.println("<p>Price: $" + rs.getDouble("price") + "</p>");
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
