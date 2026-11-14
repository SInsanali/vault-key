//! "Check for updates": asks GitHub for the latest release, only when the user presses the
//! button. Vault Key goes online for nothing else.
//!
//! The request is made by Windows' own `curl.exe` (part of Windows 10 1803 and later; the
//! installer requires 1809), run by its full path like `tasklist` and `reg`, so the app
//! needs no network code of its own. It sends nothing about the user: the address below and
//! a `User-Agent` naming the app and its version. The answer is only read for a version
//! number; the download page it points to is always [`RELEASES_PAGE`], never a link taken
//! from the answer.

use serde_json::{json, Value};

/// Where a newer version is downloaded from. Opened in the user's own browser.
pub const RELEASES_PAGE: &str = "https://github.com/SInsanali/vault-key/releases/latest";
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/SInsanali/vault-key/releases/latest";
/// GitHub's answer is a few kilobytes; anything near this is not it.
const MAX_ANSWER_BYTES: &str = "1000000";
const TIMEOUT_SECONDS: &str = "20";

const CHECK_FAILED: &str = "Couldn't check for updates. Check your internet connection, or look at the download page yourself.";

/// The commands this module answers, on a thread of their own (a check can take seconds).
pub fn handles(command: &str) -> bool {
    matches!(command, "check_for_updates" | "open_download_page")
}

pub fn run(command: &str) -> Result<Value, String> {
    match command {
        "check_for_updates" => check(),
        "open_download_page" => open_download_page().map(|()| Value::Null),
        other => Err(format!("unknown command: {other}")),
    }
}

/// `{ current, latest, newer }`.
fn check() -> Result<Value, String> {
    let answer = fetch_latest_release().map_err(|why| {
        crate::errlog::write(&format!("update check: {why}"));
        CHECK_FAILED.to_string()
    })?;
    let latest = latest_version(&answer).map_err(|why| {
        crate::errlog::write(&format!("update check: {why}"));
        CHECK_FAILED.to_string()
    })?;
    Ok(answer_json(env!("CARGO_PKG_VERSION"), latest))
}

fn answer_json(current: &str, latest: Version) -> Value {
    let newer = parse_version(current).is_some_and(|current| latest > current);
    json!({ "current": current, "latest": latest.to_string(), "newer": newer })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `1.2.3` or `v1.2.3`; nothing else (releases are tagged `v<major>.<minor>.<patch>`).
fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.').map(|part| {
        if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
            None
        } else {
            part.parse::<u64>().ok()
        }
    });
    let version = Version(parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// The version of GitHub's "latest release" answer (its `tag_name`).
fn latest_version(answer: &[u8]) -> Result<Version, String> {
    let value: Value = serde_json::from_slice(answer).map_err(|e| format!("the answer is not JSON: {e}"))?;
    let tag = value.get("tag_name").and_then(Value::as_str).ok_or("the answer has no tag_name")?;
    parse_version(tag).ok_or_else(|| format!("the latest release's tag is not a version: {tag:?}"))
}

#[cfg(windows)]
fn fetch_latest_release() -> Result<Vec<u8>, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let user_agent = concat!("VaultKey/", env!("CARGO_PKG_VERSION"));
    let output = std::process::Command::new(gkcore::guards::system_program("curl.exe"))
        .args(["--silent", "--show-error", "--fail", "--location"])
        // HTTPS only, redirects included.
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--max-time", TIMEOUT_SECONDS, "--max-filesize", MAX_ANSWER_BYTES])
        .args(["--header", "Accept: application/vnd.github+json", "--user-agent", user_agent])
        .arg(LATEST_RELEASE_API)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("could not run curl.exe: {e}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl.exe ended with {}: {}", output.status, said.trim()));
    }
    Ok(output.stdout)
}

#[cfg(not(windows))]
fn fetch_latest_release() -> Result<Vec<u8>, String> {
    Err("update checks only run on Windows".to_string())
}

/// Opens [`RELEASES_PAGE`] in the user's default browser. The window itself never leaves
/// the app's own page.
#[cfg(windows)]
fn open_download_page() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // rundll32 hands the address to whatever the user has chosen for web links.
    std::process::Command::new(gkcore::guards::system_program("rundll32.exe"))
        .args(["url.dll,FileProtocolHandler", RELEASES_PAGE])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            crate::errlog::write(&format!("opening the download page: {e}"));
            format!("Couldn't open your browser. The download page is {RELEASES_PAGE}")
        })
}

#[cfg(not(windows))]
fn open_download_page() -> Result<(), String> {
    Err(format!("The download page is {RELEASES_PAGE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_release_tags() {
        assert_eq!(parse_version("v0.9.1"), Some(Version(0, 9, 1)));
        assert_eq!(parse_version("1.10.0"), Some(Version(1, 10, 0)));
        for bad in ["", "v", "0.9", "0.9.1.2", "v0.9.1-beta", "0.9.x", "0..1", "+1.0.0", " v1.2 .3", "1234567890.0.0"] {
            assert_eq!(parse_version(bad), None, "{bad:?}");
        }
        // Numbers compare as numbers, not text.
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
    }

    #[test]
    fn githubs_answer_gives_the_latest_version() {
        let answer = br#"{"tag_name":"v0.9.1","name":"Vault Key 0.9.1","html_url":"https://example.com/elsewhere","draft":false}"#;
        assert_eq!(latest_version(answer), Ok(Version(0, 9, 1)));
        assert!(latest_version(b"<html>rate limited</html>").is_err());
        assert!(latest_version(br#"{"message":"Not Found"}"#).is_err());
        assert!(latest_version(br#"{"tag_name":"nightly"}"#).is_err());
    }

    #[test]
    fn only_a_higher_version_is_newer() {
        assert_eq!(answer_json("0.9.0", Version(0, 9, 1)), json!({ "current": "0.9.0", "latest": "0.9.1", "newer": true }));
        assert_eq!(answer_json("0.9.1", Version(0, 9, 1))["newer"], json!(false));
        // A build ahead of the last release (a development build) is not told to go back.
        assert_eq!(answer_json("0.10.0", Version(0, 9, 1))["newer"], json!(false));
    }

    #[test]
    fn only_the_two_update_commands_are_taken() {
        assert!(handles("check_for_updates") && handles("open_download_page"));
        assert!(!handles("save") && !handles("open_character"));
        assert!(run("save").is_err());
    }
}
