use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;

use axum::extract::ConnectInfo;
use axum::extract::Request;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

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

    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

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

    log_request(
        method.as_str(),
        &uri.to_string(),
        status,
        latency_ms,
        client_ip.as_deref(),
    );

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

fn log_request(method: &str, uri: &str, status: u16, latency_ms: u64, remote_addr: Option<&str>) {
    if is_static_asset(uri) {
        return;
    }

    let timestamp = chrono_now();
    let ip_part = remote_addr.unwrap_or("-");
    let line = format!(
        "{} INFO  \"{} {} HTTP/1.1\" {} {}ms {}",
        timestamp, method, uri, status, latency_ms, ip_part
    );
    eprintln!("{}", &line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", &line);
        }
    }
}

fn log_debug_details(req_headers: &str, resp_headers: &str) {
    let timestamp = chrono_now();
    let req_line = format!("{} DEBUG req: {}", timestamp, req_headers);
    let resp_line = format!("{} DEBUG res: {}", timestamp, resp_headers);
    eprintln!("{}", &req_line);
    eprintln!("{}", &resp_line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", &req_line);
            let _ = writeln!(f, "{}", &resp_line);
        }
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    let mut year = 1970_u32;
    let mut remaining = days;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 };
        if remaining < dy {
            break;
        }
        remaining -= dy;
        year += 1;
    }

    let mut month = 1_u32;
    let mut day = remaining + 1;
    for &mo in &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31] {
        let md = if month == 2 && is_leap(year) {
            mo + 1
        } else {
            mo
        };
        if day <= md {
            break;
        }
        day -= md;
        month += 1;
    }

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, h, m, s
    )
}

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
