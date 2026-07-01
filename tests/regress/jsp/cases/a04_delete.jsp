<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement("DELETE FROM temp_sessions WHERE expires < ?");
    ps.setTimestamp(1, new Timestamp(System.currentTimeMillis()));
    int deleted = ps.executeUpdate();
    out.println("Cleaned up " + deleted + " expired sessions.");
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
