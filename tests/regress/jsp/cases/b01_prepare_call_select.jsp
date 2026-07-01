<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  CallableStatement cs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    cs = conn.prepareCall("SELECT pkg.get_user(?, ?)");
    cs.setLong(1, Long.parseLong(request.getParameter("id")));
    cs.registerOutParameter(2, Types.VARCHAR);
    cs.execute();
    String name = cs.getString(2);
    out.println("<p>User: " + name + "</p>");
  } finally {
    if (cs != null) try { cs.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
