use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CHAINCORD_LOG_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if fs::create_dir_all(&path).is_ok() {
                return path;
            }
        }
    }
    let dir = default_log_dir();
    let _ = fs::create_dir_all(&dir);
    dir
}

fn default_log_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(base) = std::env::var("APPDATA") {
            if !base.trim().is_empty() {
                return PathBuf::from(base).join("com.chaincord.app").join("logs");
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("com.chaincord.app")
                    .join("logs");
            }
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("com.chaincord.app").join("logs");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("com.chaincord.app")
                    .join("logs");
            }
        }
    }
    std::env::temp_dir().join("chaincord-logs")
}

fn utc_stamp(ms: i64) -> String {
    let ms = ms.max(0) as u64;
    let secs = ms / 1000;
    let milli = ms % 1000;
    let days = secs / 86_400;
    let sod = secs % 86_400;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{milli:03}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub fn append_call(t: i64, mode: &str, peer: Option<&str>, event: &str, level: &str) {
    let stamp = utc_stamp(if t > 0 { t } else { crate::crypto::now_ms() });
    let who = peer.filter(|p| !p.is_empty()).unwrap_or("-");
    let line = format!("{stamp} [{level}] {mode} {who} {event}\n");
    write_file("call.log", &line);
}

#[allow(dead_code)]
pub fn append_raw(kind: &str, line: &str) {
    let name = if kind.is_empty() { "app.log" } else { kind };
    let stamp = utc_stamp(crate::crypto::now_ms());
    write_file(name, &format!("{stamp} {line}\n"));
}

fn write_file(name: &str, line: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = log_dir().join(safe_name(name));
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn safe_name(name: &str) -> String {
    let trimmed = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("app.log");
    if trimmed.ends_with(".log") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.log")
    }
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, default_log_dir};

    #[test]
    fn unix_epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn default_log_dir_is_not_a_drive_root() {
        let dir = default_log_dir();
        let text = dir.to_string_lossy();
        assert!(
            !text.eq_ignore_ascii_case(r"D:\chaincord-logs")
                && !text.eq_ignore_ascii_case(r"C:\chaincord-logs"),
            "{text}"
        );
    }
}
