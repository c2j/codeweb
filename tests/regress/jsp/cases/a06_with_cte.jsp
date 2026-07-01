<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement(
      "WITH ranked AS (SELECT *, ROW_NUMBER() OVER (ORDER BY created_at) rn FROM orders) " +
      "SELECT * FROM ranked WHERE rn <= ?"
    );
    ps.setInt(1, Integer.parseInt(request.getParameter("limit")));
    rs = ps.executeQuery();
    while (rs.next()) {
%>
      <div>Order #<%= rs.getInt("id") %> — <%= rs.getString("status") %></div>
<%
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
