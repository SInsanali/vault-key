//! The native Open and Save dialogs ("Open a save file…", "Export to file…").
//!
//! A dialog runs on a thread of its own, owned by the app's window, so it is modal to the
//! window without holding up the window's event loop or the API worker. The page asks with
//! `pick_open_file` / `pick_export_file` and gets the chosen path, or `null` when the user
//! cancels. Choosing a path does nothing by itself: the page then calls `open_file` or
//! `export_file`, which apply all of the core's rules.

use std::ffi::OsString;
use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde_json::Value;

/// What the page asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Pick a save file to open.
    Open,
    /// Pick where to export: a suggested file name and, if known, the folder to start in.
    Export { name: String, folder: Option<PathBuf> },
}

impl Request {
    /// The request for an API command, or `None` if the command is not a dialog.
    pub fn from_command(command: &str, args: &Value) -> Option<Request> {
        match command {
            "pick_open_file" => Some(Request::Open),
            "pick_export_file" => {
                let name = args.get("suggestedName").and_then(Value::as_str).map(clean_name).filter(|n| !n.is_empty());
                let folder = args.get("folder").and_then(Value::as_str).filter(|f| !f.is_empty()).map(PathBuf::from).filter(|f| f.is_absolute());
                Some(Request::Export { name: name.unwrap_or_else(|| "Save0002.sav".to_string()), folder })
            }
            _ => None,
        }
    }
}

/// A file name the dialog can suggest: no folders, no characters Windows refuses.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    base.chars().filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c)).collect::<String>().trim().to_string()
}

/// The folder a dialog starts in: the one asked for if it exists, else Documents.
pub fn start_folder(asked: Option<&Path>, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(folder) = asked.filter(|f| f.is_dir()) {
        return Some(folder.to_path_buf());
    }
    let profile = env("USERPROFILE").filter(|v| !v.is_empty()).map(PathBuf::from);
    let one_drive = env("OneDrive").filter(|v| !v.is_empty()).map(PathBuf::from);
    [profile, one_drive].into_iter().flatten().map(|base| base.join("Documents")).find(|dir| dir.is_dir())
}

/// The app's window, as the owner of a dialog shown from another thread.
#[derive(Clone, Copy)]
pub struct Owner(Option<NonZeroIsize>);

impl Owner {
    /// The window's handle, taken on the thread that made the window.
    pub fn of(window: &impl HasWindowHandle) -> Owner {
        let hwnd = window.window_handle().ok().and_then(|handle| match handle.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd),
            _ => None,
        });
        Owner(hwnd)
    }
}

impl HasWindowHandle for Owner {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = self.0.ok_or(HandleError::Unavailable)?;
        // SAFETY: the handle is the app's main window, which lives until the process ends;
        // the dialog that borrows it is closed before then (the app exits by ending its
        // event loop, which ends the process).
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))) })
    }
}

impl HasDisplayHandle for Owner {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

/// Only one dialog at a time.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Shows the dialog on a new thread and calls `answer` with the result: the chosen path,
/// `Ok(None)` when canceled, or `Err` with a sentence when a dialog is already open.
pub fn show(owner: Owner, request: Request, answer: impl FnOnce(Result<Option<PathBuf>, String>) + Send + 'static) {
    if OPEN.swap(true, Ordering::SeqCst) {
        answer(Err("A file dialog is already open.".to_string()));
        return;
    }
    std::thread::spawn(move || {
        let env = |key: &str| std::env::var_os(key);
        let chosen = match request {
            Request::Open => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Open a save file")
                    .add_filter("Borderlands 2 saves", &["sav"])
                    .add_filter("All files", &["*"])
                    .set_parent(&owner);
                if let Some(folder) = start_folder(None, &env) {
                    dialog = dialog.set_directory(folder);
                }
                dialog.pick_file()
            }
            Request::Export { name, folder } => {
                let mut dialog = rfd::FileDialog::new()
                    .set_title("Export to file")
                    .add_filter("Borderlands 2 saves", &["sav"])
                    .set_file_name(name)
                    .set_parent(&owner);
                if let Some(folder) = start_folder(folder.as_deref(), &env) {
                    dialog = dialog.set_directory(folder);
                }
                dialog.save_file()
            }
        };
        OPEN.store(false, Ordering::SeqCst);
        answer(Ok(chosen));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_from_the_page() {
        assert_eq!(Request::from_command("pick_open_file", &Value::Null), Some(Request::Open));
        assert_eq!(Request::from_command("open_file", &Value::Null), None);
        let export = |args: Value| Request::from_command("pick_export_file", &args).unwrap();
        assert_eq!(export(json!({ "suggestedName": "Save0002.sav" })), Request::Export { name: "Save0002.sav".into(), folder: None });
        // Only a name is suggested: no folders sneak in, nor characters Windows refuses.
        assert_eq!(export(json!({ "suggestedName": "..\\..\\x/Save0003.sav" })), Request::Export { name: "Save0003.sav".into(), folder: None });
        assert_eq!(export(json!({ "suggestedName": "a<b>?.sav" })), Request::Export { name: "ab.sav".into(), folder: None });
        assert_eq!(export(json!({})), Request::Export { name: "Save0002.sav".into(), folder: None });
        // A relative folder is ignored.
        assert_eq!(export(json!({ "suggestedName": "x.sav", "folder": "rel" })), Request::Export { name: "x.sav".into(), folder: None });
    }

    #[test]
    fn dialogs_start_in_the_folder_asked_for_or_documents() {
        let temp = std::env::temp_dir().join(format!("gk-dialogs-{}", std::process::id()));
        let docs = temp.join("Documents");
        std::fs::create_dir_all(&docs).unwrap();
        let home = temp.clone().into_os_string();
        let env = move |key: &str| (key == "USERPROFILE").then(|| home.clone());
        assert_eq!(start_folder(Some(&temp), &env), Some(temp.clone()));
        assert_eq!(start_folder(Some(&temp.join("gone")), &env), Some(docs.clone()));
        assert_eq!(start_folder(None, &env), Some(docs));
        assert_eq!(start_folder(None, &|_: &str| None), None);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
