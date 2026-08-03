use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;

use axum::extract::ConnectInfo;
use axum::extract::Request;
use axum::http::header;
use axum::http::Version;
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, FixedOffset, Local};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Debug,
}

static HTTP_LOG: Mutex<Option<File>> = Mutex::new(None);
static LOG_LEVEL: Mutex<LogLevel> = Mutex::new(LogLevel::Info);

pub fn init(log_dir: &Path, log_level: LogLevel) {
    if let Ok(mut guard) = LOG_LEVEL.lock() {
        *guard = log_level;
    }

    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join("http.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    if let Ok(mut guard) = HTTP_LOG.lock() {
        *guard = file;
    }
}

pub async fn access_log_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let uri_str = uri.to_string();
    let version = request.version();

    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    let user_agent =
        header_value_str(request.headers().get(header::USER_AGENT)).map(str::to_string);
    let referer = header_value_str(request.headers().get(header::REFERER)).map(str::to_string);

    let is_debug = LOG_LEVEL
        .lock()
        .map(|g| *g == LogLevel::Debug)
        .unwrap_or(false);

    let req_headers = if is_debug {
        extract_debug_headers(request.headers(), false)
    } else {
        None
    };

    let start = std::time::Instant::now();
    let response = next.run(request).await;
    let latency_ms = start.elapsed().as_millis() as u64;
    let status = response.status().as_u16();

    let resp_headers = if is_debug {
        extract_debug_headers(response.headers(), true)
    } else {
        None
    };

    log_request(&AccessLogEntry {
        method: method.as_str(),
        uri: &uri_str,
        version,
        status,
        latency_ms,
        remote_addr: client_ip.as_deref(),
        user_agent: user_agent.as_deref(),
        referer: referer.as_deref(),
    });

    if let (Some(req_h), Some(resp_h)) = (req_headers, resp_headers) {
        log_debug_details(&req_h, &resp_h);
    }

    response
}

/// Extract key headers for debug logging.
/// `is_response`: true = response headers (Content-Type, Content-Length),
///                false = request headers (Content-Type, Content-Length, User-Agent).
fn extract_debug_headers(headers: &axum::http::HeaderMap, is_response: bool) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ct) = headers.get(header::CONTENT_TYPE) {
        if let Ok(v) = ct.to_str() {
            parts.push(format!("Content-Type={}", v));
        }
    }
    if let Some(cl) = headers.get(header::CONTENT_LENGTH) {
        if let Ok(v) = cl.to_str() {
            parts.push(format!("Content-Length={}", v));
        }
    }
    if !is_response {
        if let Some(ua) = headers.get(header::USER_AGENT) {
            if let Ok(v) = ua.to_str() {
                parts.push(format!("User-Agent={}", v));
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn header_value_str(value: Option<&axum::http::HeaderValue>) -> Option<&str> {
    value.and_then(|v| v.to_str().ok())
}

fn is_static_asset(uri: &str) -> bool {
    // Split at '?' to get the path portion only
    let path = uri.split('?').next().unwrap_or(uri);

    if path.ends_with(".js")
        || path.ends_with(".css")
        || path.ends_with(".png")
        || path.ends_with(".ico")
        || path.ends_with(".svg")
        || path.ends_with(".woff")
        || path.ends_with(".woff2")
        || path.ends_with(".ttf")
    {
        return true;
    }

    let path = path.trim_start_matches('/');
    path.starts_with("js/")
        || path.starts_with("css/")
        || path.starts_with("img/")
        || path.starts_with("favicon")
}

/// A single HTTP request to be written to the access log.
struct AccessLogEntry<'a> {
    method: &'a str,
    uri: &'a str,
    version: Version,
    status: u16,
    latency_ms: u64,
    remote_addr: Option<&'a str>,
    user_agent: Option<&'a str>,
    referer: Option<&'a str>,
}

fn log_request(entry: &AccessLogEntry<'_>) {
    if is_static_asset(entry.uri) {
        return;
    }

    let line = format_combined_line(&now_timestamp(), entry);
    write_line(&line);
}

/// Apache Combined Log Format line.
///
/// `%h - - [%t] "%r" %s %b "%{Referer}i" "%{User-agent}i" <latency>ms`
fn format_combined_line(timestamp: &DateTime<FixedOffset>, entry: &AccessLogEntry<'_>) -> String {
    let remote = entry.remote_addr.unwrap_or("-");
    let request_line = format!(
        "{} {} {}",
        entry.method,
        entry.uri,
        http_version_str(entry.version)
    );
    format!(
        "{} - - [{}] \"{}\" {} - \"{}\" \"{}\" {}ms",
        remote,
        clf_timestamp(timestamp),
        request_line,
        entry.status,
        entry.referer.unwrap_or("-"),
        entry.user_agent.unwrap_or("-"),
        entry.latency_ms
    )
}

fn log_debug_details(req_headers: &str, resp_headers: &str) {
    let timestamp = iso_timestamp(&now_timestamp());
    let req_line = format!("{} DEBUG req: {}", timestamp, req_headers);
    let resp_line = format!("{} DEBUG res: {}", timestamp, resp_headers);
    write_line(&req_line);
    write_line(&resp_line);
}

fn write_line(line: &str) {
    eprintln!("{}", line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", line);
        }
    }
}

fn now_timestamp() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn clf_timestamp(timestamp: &DateTime<FixedOffset>) -> String {
    timestamp.format("%d/%b/%Y:%H:%M:%S %z").to_string()
}

fn iso_timestamp(timestamp: &DateTime<FixedOffset>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S %z").to_string()
}

fn http_version_str(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_ts(
        offset_secs: i32,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        min: u32,
        sec: u32,
    ) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(offset_secs)
            .unwrap()
            .with_ymd_and_hms(year, month, day, hour, min, sec)
            .unwrap()
    }

    #[test]
    fn clf_timestamp_formats_with_offset() {
        let ts = fixed_ts(8 * 3600, 2026, 7, 31, 4, 16, 31);
        assert_eq!(clf_timestamp(&ts), "31/Jul/2026:04:16:31 +0800");
    }

    #[test]
    fn clf_timestamp_utc_offset() {
        let ts = fixed_ts(0, 2026, 1, 1, 0, 0, 0);
        assert_eq!(clf_timestamp(&ts), "01/Jan/2026:00:00:00 +0000");
    }

    #[test]
    fn clf_timestamp_negative_offset() {
        let ts = fixed_ts(-5 * 3600, 2026, 12, 25, 23, 59, 59);
        assert_eq!(clf_timestamp(&ts), "25/Dec/2026:23:59:59 -0500");
    }

    #[test]
    fn http_version_rendering() {
        assert_eq!(http_version_str(Version::HTTP_09), "HTTP/0.9");
        assert_eq!(http_version_str(Version::HTTP_10), "HTTP/1.0");
        assert_eq!(http_version_str(Version::HTTP_11), "HTTP/1.1");
        assert_eq!(http_version_str(Version::HTTP_2), "HTTP/2");
        assert_eq!(http_version_str(Version::HTTP_3), "HTTP/3");
    }

    #[test]
    fn combined_line_full_format() {
        let ts = fixed_ts(8 * 3600, 2026, 7, 31, 4, 16, 31);
        let entry = AccessLogEntry {
            method: "GET",
            uri: "/api/v1/nodes/search-sql?q=dat_trd_equity",
            version: Version::HTTP_11,
            status: 200,
            latency_ms: 16070,
            remote_addr: Some("127.0.0.1"),
            user_agent: None,
            referer: None,
        };
        let line = format_combined_line(&ts, &entry);
        let expected = r#"127.0.0.1 - - [31/Jul/2026:04:16:31 +0800] "GET /api/v1/nodes/search-sql?q=dat_trd_equity HTTP/1.1" 200 - "-" "-" 16070ms"#;
        assert_eq!(line, expected);
    }

    #[test]
    fn combined_line_defaults_dash() {
        let ts = fixed_ts(0, 2026, 7, 31, 4, 16, 31);
        let entry = AccessLogEntry {
            method: "POST",
            uri: "/api/v1/query",
            version: Version::HTTP_2,
            status: 201,
            latency_ms: 5,
            remote_addr: None,
            user_agent: Some("codeweb-test"),
            referer: Some("http://localhost/"),
        };
        let line = format_combined_line(&ts, &entry);
        let expected = r#"- - - [31/Jul/2026:04:16:31 +0000] "POST /api/v1/query HTTP/2" 201 - "http://localhost/" "codeweb-test" 5ms"#;
        assert_eq!(line, expected);
    }
}
