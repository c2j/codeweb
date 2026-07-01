<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT calc_tax(?, ?)");
    ps.setDouble(1, Double.parseDouble(request.getParameter("amount")));
    ps.setString(2, request.getParameter("region"));
    ResultSet rs = ps.executeQuery();
    if (rs.next()) {
      out.println("<p>Tax: " + rs.getDouble(1) + "</p>");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
