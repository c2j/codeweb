<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT id, name FROM customers WHERE status = ?");
    ps.setString(1, request.getParameter("status"));
    rs = ps.executeQuery();
    while (rs.next()) {
      out.println("<li>" + rs.getString("name") + "</li>");
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
