<%@ page import="java.sql.*" %>
<%!
  private static final String FIND_BY_ID = "SELECT id, name FROM users WHERE id = ?";
%>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement(FIND_BY_ID);
    ps.setInt(1, Integer.parseInt(request.getParameter("id")));
    rs = ps.executeQuery();
    if (rs.next()) {
%>
      <h1><%= rs.getString("name") %></h1>
      <p>ID: <%= rs.getInt("id") %></p>
<%
    } else {
      out.println("User not found.");
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
