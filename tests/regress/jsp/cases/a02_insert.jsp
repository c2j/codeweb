<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement("INSERT INTO audit_log (user_id, action) VALUES (?, ?)");
    ps.setInt(1, Integer.parseInt(request.getParameter("userId")));
    ps.setString(2, request.getParameter("action"));
    int rows = ps.executeUpdate();
    if (rows > 0) {
      out.println("Audit log entry created successfully.");
    }
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
