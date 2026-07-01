<%@ page import="java.sql.*" %>
<%
  Connection conn = DriverManager.getConnection("jdbc:default:connection");
  PreparedStatement ps = conn.prepareStatement("SELECT id, name FROM users WHERE status = ?");
  ResultSet rs = ps.executeQuery();
  while (rs.next()) { out.println(rs.getString("name")); }
%>
