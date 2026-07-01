<%@ page import="java.sql.*" %>
<% PreparedStatement ps = conn.prepareStatement("SELECT * FROM users"); %>
<% PreparedStatement ps2 = conn.prepareStatement("SELECT * FROM users"); %>
