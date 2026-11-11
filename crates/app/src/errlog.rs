//! The error log: a small text file in the app's data folder that records panics and
//! startup failures, so someone with a problem has a file to send.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// The log's file name, inside the app's data folder.
pub const LOG_FILE_NAME: &str = "vault-key.log";
/// The log starts over once it is larger than this.
const MAX_LOG_BYTES: u64 = 256 * 1024;

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Sets where the log goes and makes every panic write to it. Call once, early.
pub fn install(data_dir: &Path) -> PathBuf {
    let path = data_dir.join(LOG_FILE_NAME);
    let _ = LOG_FILE.set(path.clone());
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&format!("panic: {info}"));
        default_hook(info);
    }));
    path
}

/// Where the log is, once [`install`] has run.
pub fn path() -> Option<&'static Path> {
    LOG_FILE.get().map(PathBuf::as_path)
}

/// Adds one entry to the log. Does nothing if the log has no place yet or cannot be written.
pub fn write(message: &str) {
    if let Some(path) = LOG_FILE.get() {
        append(path, message, SystemTime::now());
    }
}

fn append(path: &Path, message: &str, now: SystemTime) {
    let too_big = std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES);
    let mut options = OpenOptions::new();
    options.create(true);
    if too_big {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    if let Ok(mut file) = options.open(path) {
        let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "[{} UTC] Vault Key {}: {message}", timestamp(seconds), env!("CARGO_PKG_VERSION"));
    }
}

/// `seconds` since 1970 as `YYYY-MM-DD HH:MM:SS` (UTC).
fn timestamp(seconds: u64) -> String {
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_dates() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(timestamp(1_794_355_199), "2026-11-10 23:59:59");
    }

    #[test]
    fn entries_are_appended_and_a_large_log_starts_over() {
        let dir = std::env::temp_dir().join(format!("gk-errlog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOG_FILE_NAME);
        let _ = std::fs::remove_file(&path);
        let at = UNIX_EPOCH + Duration::from_secs(951_782_400);
        append(&path, "first", at);
        append(&path, "second", at);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("[2000-02-29 00:00:00 UTC] Vault Key ") && lines[0].ends_with(": first"), "{}", lines[0]);
        assert!(lines[1].ends_with(": second"));

        std::fs::write(&path, vec![b'x'; MAX_LOG_BYTES as usize + 1]).unwrap();
        append(&path, "fresh", at);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.trim_end().ends_with(": fresh"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
