<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    boolean cond = request.getParameter("mode") != null;
%>
<% if (cond) { %>
<b>Mode is enabled</b>
<% } else { %>
<i>Mode is disabled</i>
<% } %>
<%
    PreparedStatement ps = conn.prepareStatement("SELECT * FROM items WHERE mode = ?");
    ps.setBoolean(1, cond);
    ResultSet rs = ps.executeQuery();
    while (rs.next()) {
      out.println("<li>" + rs.getString("name") + "</li>");
    }
  } finally {
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
