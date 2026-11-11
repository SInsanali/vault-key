//! The few direct Windows calls the app makes: its taskbar identity, the "one copy at a
//! time" guard, and a plain message box for problems that happen before there is a page.
//!
//! Everything here has a do-nothing version for other systems, so the crate still checks
//! and tests anywhere.

use std::path::Path;

/// The app's taskbar identity (its AppUserModelID). The Start menu shortcut made by the
/// installer carries the same text (`packaging/installer.iss`), so a pinned shortcut and
/// the running window share one taskbar button.
pub const APP_USER_MODEL_ID: &str = "VaultKey.App";

/// The name of the mutex held while the app runs on the user's real folders. The installer
/// and uninstaller look for it (`AppMutex` in `packaging/installer.iss`) and ask the user to
/// close the app first.
pub const RUNNING_MUTEX: &str = "VaultKey.App.Running";

/// The mutex name for a run on the given data folder. A run on the default folder uses
/// [`RUNNING_MUTEX`] itself; a run pointed somewhere else (a sandbox, the self-test) gets a
/// name of its own, so it neither blocks nor is blocked by the user's own window.
pub fn mutex_name(data_dir: &Path, is_default_dir: bool) -> String {
    if is_default_dir {
        return RUNNING_MUTEX.to_string();
    }
    // FNV-1a over the folder's text, spelled one way (lowercase, backslashes, no trailing
    // separator): stable, and good enough to tell folders apart.
    let text = data_dir.to_string_lossy().to_lowercase().replace('/', "\\");
    let text = text.trim_end_matches('\\');
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{RUNNING_MUTEX}.{hash:016x}")
}

/// What [`claim_single_instance`] found.
pub enum Instance {
    /// This is the only copy. Keep the guard alive for as long as the app runs.
    First(InstanceGuard),
    /// Another copy is already running on the same folder; it has been asked to come to
    /// the front.
    AlreadyRunning,
}

#[cfg(windows)]
mod imp {
    use super::Instance;
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const ERROR_ALREADY_EXISTS: u32 = 183;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;
    const ASFW_ANY: u32 = 0xFFFF_FFFF;
    const MB_OK: u32 = 0;
    const MB_ICONERROR: u32 = 0x10;
    const MB_SETFOREGROUND: u32 = 0x0001_0000;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> Handle;
        fn CreateEventW(attributes: *const c_void, manual_reset: i32, initial_state: i32, name: *const u16) -> Handle;
        fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
        fn SetEvent(event: Handle) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
        fn MessageBoxW(owner: Handle, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Holds the mutex. Windows releases it when the process ends, however it ends.
    pub struct InstanceGuard(#[allow(dead_code)] Handle);
    // SAFETY: the handle is only ever held, never used, after it is created.
    unsafe impl Send for InstanceGuard {}

    struct SendHandle(Handle);
    // SAFETY: a kernel event handle may be waited on from any thread.
    unsafe impl Send for SendHandle {}

    pub fn set_app_user_model_id(id: &str) {
        let id = wide(id);
        // SAFETY: `id` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    }

    pub fn claim_single_instance(name: &str, on_wake: impl Fn() + Send + 'static) -> Instance {
        let mutex_name = wide(name);
        let event_name = wide(&format!("{name}.Wake"));
        // SAFETY: both names are NUL-terminated UTF-16 strings that outlive the calls; null
        // security attributes mean the defaults.
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            let error = GetLastError();
            // A copy running as administrator holds a mutex this copy may not open; that
            // still means it is running. It cannot be woken from here.
            if mutex.is_null() && error == ERROR_ACCESS_DENIED {
                return Instance::AlreadyRunning;
            }
            if !mutex.is_null() && error == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex);
                // The user just started this copy, so it may hand the foreground on.
                AllowSetForegroundWindow(ASFW_ANY);
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !event.is_null() {
                    SetEvent(event);
                    CloseHandle(event);
                }
                return Instance::AlreadyRunning;
            }
            // An auto-reset event: each signal wakes the waiting thread once.
            let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
            if !event.is_null() {
                let event = SendHandle(event);
                std::thread::spawn(move || {
                    let event = event;
                    while WaitForSingleObject(event.0, INFINITE) == WAIT_OBJECT_0 {
                        on_wake();
                    }
                });
            }
            Instance::First(InstanceGuard(mutex))
        }
    }

    pub fn error_box(title: &str, text: &str) {
        let (title, text) = (wide(title), wide(text));
        // SAFETY: both strings are NUL-terminated UTF-16 and outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR | MB_SETFOREGROUND) };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Instance;

    pub struct InstanceGuard;

    pub fn set_app_user_model_id(_id: &str) {}

    pub fn claim_single_instance(_name: &str, _on_wake: impl Fn() + Send + 'static) -> Instance {
        Instance::First(InstanceGuard)
    }

    pub fn error_box(title: &str, text: &str) {
        eprintln!("{title}: {text}");
    }
}

pub use imp::InstanceGuard;

/// Tells Windows which taskbar button this process belongs to. Call before the window exists.
pub fn set_app_user_model_id(id: &str) {
    imp::set_app_user_model_id(id);
}

/// Makes this the one running copy for `name`, or wakes the copy that already is.
/// `on_wake` runs (on a thread of its own) each time a later copy is started.
pub fn claim_single_instance(name: &str, on_wake: impl Fn() + Send + 'static) -> Instance {
    imp::claim_single_instance(name, on_wake)
}

/// Shows a plain error box and waits for the user to close it.
pub fn error_box(title: &str, text: &str) {
    imp::error_box(title, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_on_another_folder_has_its_own_mutex_name() {
        let default = Path::new("X:\\Users\\me\\AppData\\Local\\VaultKey");
        assert_eq!(mutex_name(default, true), RUNNING_MUTEX);
        let sandbox = mutex_name(Path::new("X:\\box\\VaultKey"), false);
        assert!(sandbox.starts_with("VaultKey.App.Running.") && sandbox.len() == RUNNING_MUTEX.len() + 17, "{sandbox}");
        // The same folder in another spelling is the same name; another folder is not.
        assert_eq!(sandbox, mutex_name(Path::new("x:\\BOX\\vaultkey"), false));
        assert_eq!(sandbox, mutex_name(Path::new("X:\\box\\VaultKey\\"), false));
        assert_eq!(sandbox, mutex_name(Path::new("X:/box/VaultKey"), false));
        assert_ne!(sandbox, mutex_name(Path::new("X:\\box2\\VaultKey"), false));
        // Mutex names may not contain a backslash.
        assert!(!sandbox.contains('\\'));
    }

    #[cfg(windows)]
    #[test]
    fn a_second_copy_wakes_the_first() {
        use std::sync::mpsc;
        let name = format!("VaultKey.Test.{}", std::process::id());
        let (woke, wakes) = mpsc::channel();
        let first = claim_single_instance(&name, move || {
            let _ = woke.send(());
        });
        assert!(matches!(first, Instance::First(_)));
        let second = claim_single_instance(&name, || {});
        assert!(matches!(second, Instance::AlreadyRunning));
        wakes.recv_timeout(std::time::Duration::from_secs(5)).expect("the first copy was woken");
    }
}
