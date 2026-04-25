use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG: Mutex<Option<File>> = Mutex::new(None);
static WARN_COUNT: Mutex<usize> = Mutex::new(0);
static ERROR_COUNT: Mutex<usize> = Mutex::new(0);

pub fn init(log_dir: &Path) {
    let _ = std::fs::create_dir_all(log_dir);
    let log_path = log_dir.join("parse.log");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .ok();

    if let Ok(mut guard) = LOG.lock() {
        *guard = file;
    }
    if let Ok(mut w) = WARN_COUNT.lock() {
        *w = 0;
    }
    if let Ok(mut e) = ERROR_COUNT.lock() {
        *e = 0;
    }
}

pub fn warn(file: &str, message: &str) {
    log_entry("WARN", file, message);
    if let Ok(mut c) = WARN_COUNT.lock() {
        *c += 1;
    }
}

#[allow(dead_code)]
pub fn error(file: &str, message: &str) {
    log_entry("ERROR", file, message);
    if let Ok(mut c) = ERROR_COUNT.lock() {
        *c += 1;
    }
}

pub fn info(file: &str, message: &str) {
    log_entry("INFO", file, message);
}

pub fn summary() -> (usize, usize) {
    let w = WARN_COUNT.lock().map(|g| *g).unwrap_or(0);
    let e = ERROR_COUNT.lock().map(|g| *g).unwrap_or(0);
    (w, e)
}

fn log_entry(level: &str, file: &str, message: &str) {
    let timestamp = chrono_now();
    let line = format!("[{}] {} {}: {}\n", level, timestamp, file, message);

    if let Ok(mut guard) = LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = f.write_all(line.as_bytes());
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
