use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;

use axum::extract::ConnectInfo;
use axum::extract::Request;
use axum::http::{header, Version};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Local;

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
    let version = version_str(request.version()).to_string();

    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    let referer = header_str(request.headers(), header::REFERER);
    let user_agent = header_str(request.headers(), header::USER_AGENT);

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
    let bytes = header_str(response.headers(), header::CONTENT_LENGTH);

    let resp_headers = if is_debug {
        extract_debug_headers(response.headers(), true)
    } else {
        None
    };

    log_request(&AccessEntry {
        remote_addr: client_ip.as_deref(),
        method: method.as_str(),
        uri: &uri.to_string(),
        version: &version,
        status,
        bytes: &bytes,
        referer: &referer,
        user_agent: &user_agent,
        latency_ms,
    });

    if let (Some(req_h), Some(resp_h)) = (req_headers, resp_headers) {
        log_debug_details(&req_h, &resp_h);
    }

    response
}

fn version_str(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2",
        Version::HTTP_3 => "HTTP/3",
        _ => "HTTP/1.1",
    }
}

fn header_str(headers: &axum::http::HeaderMap, name: axum::http::HeaderName) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string()
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

struct AccessEntry<'a> {
    remote_addr: Option<&'a str>,
    method: &'a str,
    uri: &'a str,
    version: &'a str,
    status: u16,
    bytes: &'a str,
    referer: &'a str,
    user_agent: &'a str,
    latency_ms: u64,
}

fn log_request(entry: &AccessEntry) {
    if is_static_asset(entry.uri) {
        return;
    }

    let ip_part = entry.remote_addr.unwrap_or("-");
    let timestamp = clf_timestamp();
    let line = format!(
        "{} - - [{}] \"{} {} {}\" {} {} \"{}\" \"{}\" {}ms",
        ip_part,
        timestamp,
        entry.method,
        entry.uri,
        entry.version,
        entry.status,
        entry.bytes,
        entry.referer,
        entry.user_agent,
        entry.latency_ms
    );
    eprintln!("{}", line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", line);
        }
    }
}

fn log_debug_details(req_headers: &str, resp_headers: &str) {
    let timestamp = debug_timestamp();
    let req_line = format!("{} DEBUG req: {}", timestamp, req_headers);
    let resp_line = format!("{} DEBUG res: {}", timestamp, resp_headers);
    eprintln!("{}", req_line);
    eprintln!("{}", resp_line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", req_line);
            let _ = writeln!(f, "{}", resp_line);
        }
    }
}

/// Apache CLF timestamp: [dd/Mon/yyyy:HH:MM:SS +ZZZZ]
fn clf_timestamp() -> String {
    Local::now().format("%d/%b/%Y:%H:%M:%S %z").to_string()
}

fn debug_timestamp() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}
