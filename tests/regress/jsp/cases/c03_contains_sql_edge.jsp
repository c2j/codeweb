<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT id, description FROM tasks WHERE done = ?");
    ps.setBoolean(1, false);
    ResultSet rs = ps.executeQuery();
    while (rs.next()) {
      out.println("<li>" + rs.getString("description") + "</li>");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
