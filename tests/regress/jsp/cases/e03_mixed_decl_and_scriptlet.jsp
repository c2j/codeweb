<%@ page import="java.sql.*" %>
<%!
  private static final String USER_SQL = "SELECT name FROM users WHERE id = ?";
%>
<%
  Connection conn = null;
  PreparedStatement ps1 = null;
  PreparedStatement ps2 = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps1 = conn.prepareStatement(USER_SQL);
    ps1.setLong(1, Long.parseLong(request.getParameter("id")));
    ResultSet rs = ps1.executeQuery();
    if (rs.next()) {
      out.println("<h2>" + rs.getString("name") + "</h2>");
    }
    ps2 = conn.prepareStatement("SELECT price FROM products WHERE id = ?");
    ps2.setLong(1, Long.parseLong(request.getParameter("pid")));
    ResultSet rs2 = ps2.executeQuery();
    if (rs2.next()) {
      out.println("<p>Price: $" + rs2.getDouble("price") + "</p>");
    }
  } finally {
    if (ps1 != null) try { ps1.close(); } catch (SQLException e) {}
    if (ps2 != null) try { ps2.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
