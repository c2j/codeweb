<%@ page import="java.sql.*" %>
<%-- Known limitation: JDBC escape syntax {call ...} is filtered by the keyword gate. --%>
<%-- Only SELECT/INSERT/UPDATE/DELETE/MERGE/WITH pass through. --%>
<%
  Connection conn = null;
  CallableStatement cs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    cs = conn.prepareCall("{call pkg.update_status(?, ?)}");
    cs.setLong(1, Long.parseLong(request.getParameter("id")));
    cs.setString(2, request.getParameter("status"));
    cs.execute();
    out.println("<p>Status updated</p>");
  } finally {
    if (cs != null) try { cs.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
