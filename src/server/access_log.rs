use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

static HTTP_LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn init(log_dir: &Path) {
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

    let start = std::time::Instant::now();
    let response = next.run(request).await;
    let latency_ms = start.elapsed().as_millis() as u64;
    let status = response.status().as_u16();

    log_request(method.as_str(), &uri.to_string(), status, latency_ms, None);

    response
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

fn log_request(
    method: &str,
    uri: &str,
    status: u16,
    latency_ms: u64,
    _remote_addr: Option<String>,
) {
    if is_static_asset(uri) {
        return;
    }

    let timestamp = chrono_now();
    let line = format!(
        "{} INFO  \"{} {} HTTP/1.1\" {} {}ms",
        timestamp, method, uri, status, latency_ms
    );
    eprintln!("{}", &line);

    if let Ok(mut guard) = HTTP_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", &line);
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
