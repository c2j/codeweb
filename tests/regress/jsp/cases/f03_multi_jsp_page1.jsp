<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT get_customer(?)");
    ps.setLong(1, Long.parseLong(request.getParameter("cid")));
    ResultSet rs = ps.executeQuery();
    if (rs.next()) {
      out.println("<p>Customer: " + rs.getString(1) + "</p>");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
