<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    ps = conn.prepareStatement("SELECT id, title, created_at FROM posts WHERE author_id = ?");
    ps.setLong(1, Long.parseLong(request.getParameter("author_id")));
    ResultSet rs = ps.executeQuery();
    while (rs.next()) {
      out.println("<li>" + rs.getString("title") + "</li>");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
