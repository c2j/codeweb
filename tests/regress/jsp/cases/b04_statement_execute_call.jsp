<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  Statement stmt = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");
    stmt = conn.createStatement();
    rs = stmt.executeQuery("SELECT proc_from_stmt()");
    if (rs.next()) {
      out.println("<p>Result: " + rs.getString(1) + "</p>");
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (stmt != null) try { stmt.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
