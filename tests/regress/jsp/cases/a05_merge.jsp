<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement(
      "MERGE INTO inventory t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET qty = s.qty"
    );
    int rows = ps.executeUpdate();
    out.println("Merge completed, affected " + rows + " rows.");
  } finally {
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
