<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT process_order(?)");
    ps.setLong(1, Long.parseLong(request.getParameter("id")));
    ps.execute();
    out.println("<p>Order processed successfully</p>");
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
