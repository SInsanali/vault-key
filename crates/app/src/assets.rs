//! The UI's files. Normally the copies embedded at build time; when `GK_UI_DIR` is set, files
//! are read from that folder instead so UI changes show up on reload without a rebuild.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

pub const UI_DIR_ENV: &str = "GK_UI_DIR";

pub fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Only plain relative paths: no `..`, drive letters or absolute paths.
fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() { None } else { Some(out) }
}

/// The bytes of a UI file, by its path relative to `/ui` (`""` means `index.html`).
pub fn load(path: &str, live_dir: Option<&Path>) -> Option<Cow<'static, [u8]>> {
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(dir) = live_dir {
        let rel = safe_relative(path)?;
        return std::fs::read(dir.join(rel)).ok().map(Cow::Owned);
    }
    ASSETS.iter().find(|(name, _)| *name == path).map(|(_, bytes)| Cow::Borrowed(*bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_is_embedded_and_typed() {
        assert!(load("", None).is_some());
        assert!(load("index.html", None).is_some());
        assert!(load("nope.html", None).is_none());
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("fonts/Bangers-Regular.ttf"), "font/ttf");
    }

    #[test]
    fn the_real_ui_is_embedded_and_the_dev_folder_is_not() {
        for file in ["css/app.css", "js/main.js", "js/api.js", "js/selftest.js", "js/views/gear.js", "fonts/BarlowCondensed-Bold.ttf"] {
            assert!(load(file, None).is_some(), "{file}");
        }
        assert_eq!(content_type("js/main.js"), "text/javascript; charset=utf-8");
        // The mock API and sample data are for working on the UI in a browser only.
        assert!(ASSETS.iter().all(|(name, _)| !name.starts_with("dev/")));
        assert!(load("dev/mock-api.js", None).is_none());
        // Nothing in the UI may load from the network.
        for (name, bytes) in ASSETS.iter().filter(|(name, _)| !name.starts_with("fonts/")) {
            // (The SVG namespace is a name, not an address that is fetched.)
            let text = String::from_utf8_lossy(bytes).replace("http://www.w3.org/2000/svg", "");
            assert!(!text.contains("https://") && !text.contains("http://") && !text.contains("url(//"), "{name}");
        }
    }

    #[test]
    fn live_folder_cannot_be_escaped() {
        let dir = std::env::temp_dir();
        for bad in ["../secret.txt", "..\\secret.txt", "C:\\Windows\\win.ini", "/etc/passwd", "a/../../b"] {
            assert!(load(bad, Some(&dir)).is_none(), "{bad}");
        }
    }
}
