<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  PreparedStatement ps = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:postgresql://localhost/mydb", "user", "pass");
    ps = conn.prepareStatement("SELECT id, name, email FROM users WHERE status = ?");
    ps.setString(1, "active");
    rs = ps.executeQuery();
    while (rs.next()) {
      int id = rs.getInt("id");
      String name = rs.getString("name");
      String email = rs.getString("email");
%>
      <tr>
        <td><%= id %></td>
        <td><%= name %></td>
        <td><%= email %></td>
      </tr>
<%
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (ps != null) try { ps.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
