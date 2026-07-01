<%@ page import="java.sql.*" %>
<%
  Connection conn = null;
  CallableStatement cs = null;
  Statement stmt = null;
  ResultSet rs = null;
  try {
    conn = DriverManager.getConnection("jdbc:default:connection");

    cs = conn.prepareCall("SELECT pkg.calc_total(?)");
    cs.setLong(1, Long.parseLong(request.getParameter("order_id")));
    cs.execute();
    double total = cs.getDouble(1);
    out.println("<p>Total: " + total + "</p>");

    stmt = conn.createStatement();
    rs = stmt.executeQuery("SELECT pkg.get_last_order()");
    if (rs.next()) {
      out.println("<p>Last order: " + rs.getLong(1) + "</p>");
    }
  } finally {
    if (rs != null) try { rs.close(); } catch (SQLException e) {}
    if (stmt != null) try { stmt.close(); } catch (SQLException e) {}
    if (cs != null) try { cs.close(); } catch (SQLException e) {}
    if (conn != null) try { conn.close(); } catch (SQLException e) {}
  }
%>
