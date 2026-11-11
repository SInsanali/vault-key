//! Vault Key: a native window around the web UI in `/ui`.
//!
//! The UI is served from inside the program over a private `gk://` scheme, and calls the
//! Rust side with `fetch("/api/<command>")`. Commands run on one worker thread that owns all
//! app state, so the window never blocks. Weapon pictures (`/picture/<size>/<item code>`) are
//! drawn on a second thread of their own.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod assets;
mod dialogs;
mod errlog;
mod icon;
mod pictures;
mod platform;

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Icon, WindowBuilder};
use wry::http::{Request, Response, StatusCode};
use wry::{RequestAsyncResponder, WebContext, WebViewBuilder};

/// Set (to anything) to make the app run its self-test and close. See [`selftest_sandbox`].
const SELFTEST_ENV: &str = "GK_SELFTEST";
/// For development: an item reference such as `54:0`. The window then opens the first
/// character with that item selected on the Gear tab (used to capture the window).
const SELECT_ENV: &str = "GK_UI_SELECT";
/// For development: the full path of a save file to open at start, as "Open a save file…"
/// would (used to capture the window with a file from outside the save folder open).
const OPEN_ENV: &str = "GK_UI_OPEN";
/// How long the web view may keep a picture. The page puts a per-run token in the address,
/// so nothing kept here outlives the run that drew it.
const PICTURE_CACHE_CONTROL: &str = "private, max-age=86400";

enum UserEvent {
    Quit,
    /// Another copy of the app was started: come to the front instead.
    ComeToFront,
}

/// Where the WebView2 Runtime can be downloaded (Microsoft's own short link).
const WEBVIEW2_DOWNLOAD: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Says why the app cannot start, in the log and in a message box, and ends the program.
fn fail_to_start(what: &str, error: &dyn std::fmt::Display, advice: &str) -> ! {
    errlog::write(&format!("{what}: {error}"));
    let log = errlog::path().map(|path| format!("\n\nDetails are in {}", path.display())).unwrap_or_default();
    platform::error_box("Vault Key", &format!("{what}.\n\n{advice}{log}"));
    std::process::exit(1);
}

/// Closing the window asks the page first, so that unsaved changes are not dropped without
/// a word and a save that is being written finishes. The page answers by calling `quit`.
/// A second close request this soon after the first closes the window whatever the page says.
const CLOSE_AGAIN_WITHIN: Duration = Duration::from_secs(5);
const ASK_PAGE_TO_CLOSE: &str = r#"(window.gkCloseRequested || (() => fetch("/api/quit", { method: "POST" })))()"#;

struct ApiCall {
    command: String,
    body: Vec<u8>,
    responder: RequestAsyncResponder,
}

fn respond(status: StatusCode, content_type: &str, body: Cow<'static, [u8]>) -> Response<Cow<'static, [u8]>> {
    respond_cached(status, content_type, "no-store", body)
}

fn respond_cached(status: StatusCode, content_type: &str, cache: &str, body: Cow<'static, [u8]>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", cache)
        .body(body)
        .expect("valid response")
}

/// A picture, cacheable, or the sentence that says why there is none (never cached).
fn picture_response(answer: pictures::Answer) -> Response<Cow<'static, [u8]>> {
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let cache = if answer.is_picture() { PICTURE_CACHE_CONTROL } else { "no-store" };
    respond_cached(status, answer.content_type, cache, Cow::Owned(answer.body))
}

/// The picture thread: answers requests one at a time, the detail picture first.
fn serve_pictures(inbox: mpsc::Receiver<(String, RequestAsyncResponder)>) {
    let mut service = pictures::Service::live();
    let mut waiting: Vec<(String, RequestAsyncResponder)> = Vec::new();
    loop {
        if waiting.is_empty() {
            match inbox.recv() {
                Ok(request) => waiting.push(request),
                Err(_) => return,
            }
        }
        waiting.extend(inbox.try_iter());
        let routes: Vec<&str> = waiting.iter().map(|(route, _)| route.as_str()).collect();
        let Some(next) = pictures::next_request(&routes) else { continue };
        let (route, responder) = waiting.remove(next);
        responder.respond(picture_response(service.answer(&route)));
    }
}

fn json_response(status: StatusCode, value: &serde_json::Value) -> Response<Cow<'static, [u8]>> {
    respond(status, "application/json", Cow::Owned(value.to_string().into_bytes()))
}

type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

fn env_path(env: Env, key: &str) -> Option<PathBuf> {
    env(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where the app keeps its own files: `GK_DATA_DIR` if set, else `%LOCALAPPDATA%\VaultKey`.
/// The same folder the core uses for settings and backups.
fn app_data_dir(env: Env) -> PathBuf {
    env_path(env, gkcore::DATA_DIR_ENV).unwrap_or_else(|| {
        let base = env_path(env, "LOCALAPPDATA").unwrap_or_else(std::env::temp_dir);
        base.join(gkcore::APP_FOLDER_NAME)
    })
}

/// The app was called Golden Key until 0.9.0 and kept its files in `%LOCALAPPDATA%\\GoldenKey`.
/// The settings (locks, test slots, theme) and the backups are copied into the new folder,
/// once: a marker file is written only after everything was copied, so a copy that fails
/// halfway is tried again on the next start. Nothing in the new folder is overwritten, and
/// the old folder is left as it is. The item cache and the window's own files are not
/// copied (they are rebuilt). Only for the default folder.
fn migrate_old_data(env: Env, new_dir: &std::path::Path) -> Result<bool, String> {
    let Some(local) = env_path(env, "LOCALAPPDATA") else { return Ok(false) };
    let old_dir = local.join(gkcore::OLD_APP_FOLDER_NAME);
    let marker = new_dir.join(MIGRATED_MARKER);
    if !old_dir.is_dir() || marker.exists() {
        return Ok(false);
    }
    let copy = || -> std::io::Result<()> {
        std::fs::create_dir_all(new_dir)?;
        copy_tree(&old_dir.join("backups"), &new_dir.join("backups"))?;
        let settings = old_dir.join(gkcore::settings::SETTINGS_FILE_NAME);
        let new_settings = new_dir.join(gkcore::settings::SETTINGS_FILE_NAME);
        if settings.is_file() && !new_settings.exists() {
            std::fs::copy(&settings, &new_settings)?;
        }
        std::fs::write(&marker, format!("Settings and backups were copied here from {}.\n", old_dir.display()))
    };
    copy().map(|()| true).map_err(|error| format!("could not copy {} to {}: {error}", old_dir.display(), new_dir.display()))
}

/// Makes the folder if needed and checks that a file can be written in it.
fn usable_folder(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-check-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// Written in the data folder once the old folder's files are copied in.
const MIGRATED_MARKER: &str = "copied-from-GoldenKey.txt";

/// Copies a folder and everything in it, leaving files that are already there alone. A
/// missing source copies nothing.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() && !to.join(entry.file_name()).exists() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// The self-test drives the app by itself: it creates a character, saves and restores. So it
/// runs only on a sandbox: both the save folder and the data folder must be given, and
/// neither may be (or be inside, or hold) a real save folder. `Ok` is the data folder, where
/// the self-test's log goes; `Err` says what is wrong.
fn selftest_sandbox(env: Env) -> Result<PathBuf, String> {
    let saves = env_path(env, gkcore::guards::SAVEDATA_ENV)
        .ok_or_else(|| format!("{} is not set", gkcore::guards::SAVEDATA_ENV))?;
    let data = env_path(env, gkcore::DATA_DIR_ENV).ok_or_else(|| format!("{} is not set", gkcore::DATA_DIR_ENV))?;
    if !saves.is_absolute() || !data.is_absolute() {
        return Err("the sandbox folders are not given as full paths".to_string());
    }
    let real = gkcore::guards::real_save_data_candidates(env);
    let touches =
        |dir: &PathBuf| real.iter().any(|root| gkcore::guards::is_inside(dir, root) || gkcore::guards::is_inside(root, dir));
    if touches(&saves) {
        return Err(format!("{} points at the real save folder", gkcore::guards::SAVEDATA_ENV));
    }
    if touches(&data) {
        return Err(format!("{} points at the real save folder", gkcore::DATA_DIR_ENV));
    }
    Ok(data)
}

/// True when `dir` is inside a SaveData folder (Steam Cloud syncs those) or a Steam library.
fn is_in_game_folders(env: Env, dir: &std::path::Path) -> bool {
    let mut save_roots = gkcore::guards::real_save_data_candidates(env);
    save_roots.extend(gkcore::guards::save_data_root(env).ok());
    save_roots.iter().any(|root| gkcore::guards::is_inside(dir, root))
        || gkcore::guards::physical_path(dir)
            .components()
            .any(|part| part.as_os_str().to_string_lossy().eq_ignore_ascii_case("steamapps"))
}

fn main() {
    // `GK_SELFTEST=1` makes the page run its self-test and then close the app.
    let env = |key: &str| std::env::var_os(key);
    let selftest_dir = if env(SELFTEST_ENV).is_some() {
        match selftest_sandbox(&env) {
            Ok(dir) => Some(dir),
            Err(why) => {
                eprintln!("The self-test only runs on a sandbox save folder, and {why}. Nothing was started.");
                std::process::exit(2);
            }
        }
    } else {
        None
    };
    let selftest = selftest_dir.is_some();
    // The core refuses a data folder inside a save folder or a Steam library (and says so in
    // the window). The window's own files must not go there either, so they move aside.
    let data_dir = Some(app_data_dir(&env))
        .filter(|dir| !is_in_game_folders(&env, dir))
        .unwrap_or_else(|| std::env::temp_dir().join(gkcore::APP_FOLDER_NAME));
    platform::set_app_user_model_id(platform::APP_USER_MODEL_ID);
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // One copy at a time per data folder: two windows could edit the same save. A second
    // start brings the first window to the front and ends. This comes before anything is
    // written to the data folder, so two quick starts never work on it at once.
    let on_default_folders = [gkcore::DATA_DIR_ENV, gkcore::guards::SAVEDATA_ENV].iter().all(|key| env_path(&env, key).is_none());
    let waker = event_loop.create_proxy();
    let _running = match platform::claim_single_instance(&platform::mutex_name(&data_dir, on_default_folders), move || {
        let _ = waker.send_event(UserEvent::ComeToFront);
    }) {
        platform::Instance::First(guard) => guard,
        platform::Instance::AlreadyRunning => return,
    };

    let migrated = if env_path(&env, gkcore::DATA_DIR_ENV).is_none() { Some(migrate_old_data(&env, &data_dir)) } else { None };
    // The window cannot start without a folder it can write (WebView2 keeps its files there,
    // and waits forever instead of failing when it cannot), so check that first.
    if let Err(error) = usable_folder(&data_dir) {
        let what = format!("Vault Key cannot use its data folder, {}", data_dir.display());
        platform::error_box("Vault Key", &format!("{what}.\n\n{error}"));
        std::process::exit(1);
    }
    errlog::install(&data_dir);
    match migrated {
        Some(Ok(true)) => errlog::write("copied the settings and backups that were in the GoldenKey folder"),
        Some(Err(why)) => errlog::write(&why),
        _ => {}
    }
    let catalog_cache = data_dir.join("GameData");
    // The color theme, so the window's first frame is the right color (read only; a
    // missing or unreadable settings file means the default theme).
    let theme = gkcore::Settings::load(&data_dir.join(gkcore::settings::SETTINGS_FILE_NAME))
        .map(|s| s.theme().to_string())
        .unwrap_or_default();
    let live_ui: Option<PathBuf> = std::env::var_os(assets::UI_DIR_ENV).map(PathBuf::from);

    // The worker owns the app state and answers API calls in order.
    let (calls, inbox) = mpsc::channel::<ApiCall>();
    std::thread::spawn(move || {
        let mut api = api::Api::live(catalog_cache, selftest_dir);
        for call in inbox {
            let args = if call.body.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                serde_json::from_slice(&call.body)
            };
            let (result, effect) = match args {
                Ok(args) => api.handle(&call.command, &args),
                Err(e) => (Err(format!("bad request body: {e}")), api::Effect::None),
            };
            let response = match result {
                Ok(value) => json_response(StatusCode::OK, &serde_json::json!({ "ok": value })),
                Err(message) => json_response(StatusCode::OK, &serde_json::json!({ "error": message })),
            };
            call.responder.respond(response);
            if effect == api::Effect::Quit {
                let _ = proxy.send_event(UserEvent::Quit);
            }
        }
    });

    // Pictures are drawn on their own thread: the first one opens the game's files, and a
    // large one takes a moment.
    let (picture_requests, picture_inbox) = mpsc::channel::<(String, RequestAsyncResponder)>();
    std::thread::spawn(move || serve_pictures(picture_inbox));

    let screen = event_loop.primary_monitor().map(|m| {
        let size = m.size().to_logical::<f64>(m.scale_factor());
        (size.width, size.height)
    });
    let (width, height) = start_size(screen);
    let window = WindowBuilder::new()
        .with_title("Vault Key")
        .with_inner_size(LogicalSize::new(width, height))
        .with_min_inner_size(LogicalSize::new(900.0, 600.0))
        .with_window_icon(Icon::from_rgba(icon::rgba(64), 64, 64).ok())
        .build(&event_loop)
        .unwrap_or_else(|error| fail_to_start("Vault Key could not open its window", &error, "Restarting Windows may help."));

    let text_env = |key: &str| env(key).and_then(|v| v.into_string().ok());
    let start_url = start_url(selftest, text_env(SELECT_ENV).as_deref(), text_env(OPEN_ENV).as_deref());
    let owner = dialogs::Owner::of(&window);
    let mut web_context = WebContext::new(Some(data_dir.join("WebView")));
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_background_color(window_background(&theme))
        .with_devtools(cfg!(debug_assertions))
        .with_asynchronous_custom_protocol("gk".into(), move |_id, request: Request<Vec<u8>>, responder| {
            let path = request.uri().path().trim_start_matches('/').to_string();
            if let Some(command) = path.strip_prefix("api/") {
                // The Open and Save dialogs run on threads of their own, so that neither the
                // window nor the API worker waits while the user picks a file.
                let args: serde_json::Value = serde_json::from_slice(request.body()).unwrap_or(serde_json::Value::Null);
                if let Some(dialog) = dialogs::Request::from_command(command, &args) {
                    dialogs::show(owner, dialog, move |chosen| {
                        let answer = match chosen {
                            Ok(path) => serde_json::json!({ "ok": path.map(|p| p.display().to_string()) }),
                            Err(message) => serde_json::json!({ "error": message }),
                        };
                        responder.respond(json_response(StatusCode::OK, &answer));
                    });
                    return;
                }
                let call = ApiCall { command: command.to_string(), body: request.into_body(), responder };
                if let Err(mpsc::SendError(call)) = calls.send(call) {
                    let gone = serde_json::json!({ "error": "the app is shutting down" });
                    call.responder.respond(json_response(StatusCode::OK, &gone));
                }
                return;
            }
            if let Some(route) = path.strip_prefix("picture/") {
                if let Err(mpsc::SendError((_, responder))) = picture_requests.send((route.to_string(), responder)) {
                    responder.respond(respond(StatusCode::SERVICE_UNAVAILABLE, "text/plain; charset=utf-8", Cow::Borrowed(b"The app is shutting down.")));
                }
                return;
            }
            match assets::load(&path, live_ui.as_deref()) {
                Some(bytes) => responder.respond(respond(StatusCode::OK, assets::content_type(&path_or_index(&path)), bytes)),
                None => responder.respond(respond(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", Cow::Borrowed(b"not found"))),
            }
        })
        .with_url(start_url)
        .build(&window)
        .unwrap_or_else(|error| {
            let advice = format!("Vault Key needs the Microsoft Edge WebView2 Runtime. Install it from\n{WEBVIEW2_DOWNLOAD}\nand start Vault Key again.");
            fail_to_start("Vault Key could not show its window", &error, &advice)
        });

    let mut close_asked: Option<Instant> = None;
    event_loop.run(move |event, _, control_flow| {
        let _keep_alive = &window;
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Quit) => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::ComeToFront) => {
                window.set_minimized(false);
                window.set_visible(true);
                window.set_focus();
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                let insisted = close_asked.is_some_and(|at| at.elapsed() < CLOSE_AGAIN_WITHIN);
                if insisted || webview.evaluate_script(ASK_PAGE_TO_CLOSE).is_err() {
                    *control_flow = ControlFlow::Exit;
                } else {
                    close_asked = Some(Instant::now());
                }
            }
            _ => {}
        }
    });
}

/// The page to open. `select` (development only) is an item reference to open selected; only
/// digits and a colon are let through.
/// The window's background before the page draws: each theme's ground color (ui/css/app.css).
fn window_background(theme: &str) -> (u8, u8, u8, u8) {
    match theme {
        "blue" => (0x0B, 0x1A, 0x2C, 255),
        "purple" => (0x1A, 0x10, 0x28, 255),
        "red" => (0x16, 0x19, 0x1D, 255),
        _ => (0x1B, 0x14, 0x0D, 255),
    }
}

/// The window's size when it opens: 1600 x 1000, but never more than 90% of the screen, and
/// never less than the smallest size the app is laid out for (900 x 600).
const START_SIZE: (f64, f64) = (1600.0, 1000.0);
fn start_size(screen: Option<(f64, f64)>) -> (f64, f64) {
    let Some((w, h)) = screen else { return START_SIZE };
    ((w * 0.9).round().min(START_SIZE.0).max(900.0), (h * 0.9).round().min(START_SIZE.1).max(600.0))
}

fn start_url(selftest: bool, select: Option<&str>, open: Option<&str>) -> String {
    if selftest {
        return "gk://localhost/index.html?selftest".to_string();
    }
    let select = select.filter(|s| !s.is_empty() && s.len() <= 12 && s.chars().all(|c| c.is_ascii_digit() || c == ':'));
    let open = open.filter(|p| !p.is_empty() && p.len() <= 1024);
    let mut query: Vec<String> = Vec::new();
    if let Some(path) = open {
        let encoded: String = path.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        query.push(format!("open={encoded}"));
    }
    if let Some(item) = select {
        query.push(format!("select={item}"));
    }
    if query.is_empty() {
        "gk://localhost/index.html".to_string()
    } else {
        format!("gk://localhost/index.html?{}", query.join("&"))
    }
}

fn path_or_index(path: &str) -> String {
    if path.is_empty() { "index.html".to_string() } else { path.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key: &str| pairs.iter().find(|(name, _)| *name == key).map(|(_, value)| OsString::from(value))
    }

    #[test]
    fn the_start_page_and_picture_responses() {
        assert_eq!(start_url(false, None, None), "gk://localhost/index.html");
        assert_eq!(start_url(true, Some("54:0"), None), "gk://localhost/index.html?selftest");
        assert_eq!(start_url(false, Some("54:0"), None), "gk://localhost/index.html?select=54:0");
        for bad in ["", "54:0&mock", "<script>", "1234567890123"] {
            assert_eq!(start_url(false, Some(bad), None), "gk://localhost/index.html", "{bad}");
        }
        assert_eq!(start_url(false, Some("54:0"), Some("C:\\x y\\S.sav")), "gk://localhost/index.html?open=C%3A%5Cx%20y%5CS%2Esav&select=54:0");
        assert_eq!(start_url(true, None, Some("C:\\S.sav")), "gk://localhost/index.html?selftest");

        // A picture may be kept by the web view; a refusal never is.
        let mut service = pictures::Service::new(Box::new(|| Err(gamemesh::PictureError::NotAWeapon)));
        let refusal = picture_response(service.answer("detail/BL2(x)"));
        assert_eq!((refusal.status(), refusal.headers()["Cache-Control"].to_str().unwrap()), (StatusCode::NOT_FOUND, "no-store"));
        assert_eq!(refusal.body().as_ref(), gamemesh::PictureError::NotAWeapon.message().as_bytes());
        let png = picture_response(pictures::Answer { status: 200, content_type: "image/png", body: vec![1, 2, 3] });
        assert_eq!((png.status(), png.headers()["Content-Type"].to_str().unwrap()), (StatusCode::OK, "image/png"));
        assert_eq!(png.headers()["Cache-Control"].to_str().unwrap(), PICTURE_CACHE_CONTROL);
    }

    #[test]
    fn each_theme_has_its_own_window_background() {
        assert_eq!(window_background("yellow"), (0x1B, 0x14, 0x0D, 255));
        assert_eq!(window_background(""), (0x1B, 0x14, 0x0D, 255));
        for theme in gkcore::settings::THEMES {
            assert!(theme == "yellow" || window_background(theme) != window_background("yellow"), "{theme}");
        }
    }

    #[test]
    fn the_window_opens_at_1600_by_1000_unless_the_screen_is_smaller() {
        assert_eq!(start_size(Some((5120.0, 1440.0))), (1600.0, 1000.0));
        assert_eq!(start_size(None), (1600.0, 1000.0));
        assert_eq!(start_size(Some((1366.0, 768.0))), (1229.0, 691.0));
        assert_eq!(start_size(Some((800.0, 600.0))), (900.0, 600.0));
    }

    #[cfg(windows)]
    #[test]
    fn the_selftest_only_runs_on_a_sandbox() {
        const REAL: &str = "X:\\Users\\me\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData";
        // Both folders given, both away from the real save folder: allowed.
        let ok = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", "X:\\box\\SaveData"), ("GK_DATA_DIR", "X:\\box\\VaultKey")]);
        assert_eq!(selftest_sandbox(&ok), Ok(PathBuf::from("X:\\box\\VaultKey")));

        // Either folder missing or empty: refused, so it can never fall back to the real ones.
        let none = env_of(&[("USERPROFILE", "X:\\Users\\me")]);
        assert!(selftest_sandbox(&none).unwrap_err().contains("GK_SAVEDATA_DIR is not set"));
        let no_data = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", "X:\\box\\SaveData")]);
        assert!(selftest_sandbox(&no_data).unwrap_err().contains("GK_DATA_DIR is not set"));
        let empty = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", ""), ("GK_DATA_DIR", "X:\\box\\VaultKey")]);
        assert!(selftest_sandbox(&empty).unwrap_err().contains("GK_SAVEDATA_DIR is not set"));
        let relative = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", "SaveData"), ("GK_DATA_DIR", "X:\\box\\VaultKey")]);
        assert!(selftest_sandbox(&relative).unwrap_err().contains("full paths"));

        // The real save folder itself, in any spelling, anything inside it, or a folder that
        // holds it: refused, for the save folder and for the data folder alike.
        for saves in [REAL, "x:\\users\\ME\\documents\\my games\\borderlands 2\\willowgame\\savedata", "X:\\Users\\me\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData\\76561190000000000", "X:\\Users\\me\\Documents"] {
            let pairs: &'static [(&str, &str)] = Box::leak(Box::new([("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", saves), ("GK_DATA_DIR", "X:\\box\\VaultKey")]));
            assert!(selftest_sandbox(&env_of(pairs)).unwrap_err().contains("GK_SAVEDATA_DIR points at the real save folder"), "{saves}");
        }
        let data_inside = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", "X:\\box\\SaveData"), ("GK_DATA_DIR", "X:\\Users\\me\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData\\gk")]);
        assert!(selftest_sandbox(&data_inside).unwrap_err().contains("GK_DATA_DIR points at the real save folder"));
        // OneDrive can hold the Documents folder; that copy counts as real too.
        let one_drive = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("OneDrive", "X:\\Cloud"), ("GK_SAVEDATA_DIR", "X:\\Cloud\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData"), ("GK_DATA_DIR", "X:\\box\\VaultKey")]);
        assert!(selftest_sandbox(&one_drive).is_err());
    }

    #[test]
    fn a_data_folder_must_be_a_writable_folder() {
        let base = std::env::temp_dir().join(format!("gk-usable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert!(usable_folder(&base.join("new").join("deeper")).is_ok());
        assert_eq!(std::fs::read_dir(base.join("new").join("deeper")).unwrap().count(), 0, "the check leaves nothing behind");
        std::fs::write(base.join("a-file"), b"x").unwrap();
        assert!(usable_folder(&base.join("a-file")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_and_backups_are_copied_once_from_the_old_folder() {
        let local = std::env::temp_dir().join(format!("gk-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&local);
        let old = local.join("GoldenKey");
        let backup = std::path::Path::new("765").join("Save0004.sav").join("20261110-000000Z");
        std::fs::create_dir_all(old.join("backups").join(&backup)).unwrap();
        std::fs::create_dir_all(old.join("GameData")).unwrap();
        std::fs::write(old.join("settings.txt"), "protected abc Someone\n").unwrap();
        std::fs::write(old.join("backups").join(&backup).join("Save0004.sav"), b"save").unwrap();
        std::fs::write(old.join("GameData").join("cache.bin"), b"cache").unwrap();
        let local_text = local.clone().into_os_string();
        let env = move |key: &str| (key == "LOCALAPPDATA").then(|| local_text.clone());
        let new = local.join("VaultKey");

        // A new folder that already exists (made by a copy that failed halfway, say) is
        // filled in, without overwriting what is there.
        std::fs::create_dir_all(new.join("backups").join(&backup)).unwrap();
        std::fs::write(new.join("backups").join(&backup).join("Save0004.sav"), b"newer").unwrap();
        assert_eq!(migrate_old_data(&env, &new), Ok(true));
        assert_eq!(std::fs::read_to_string(new.join("settings.txt")).unwrap(), "protected abc Someone\n");
        assert_eq!(std::fs::read(new.join("backups").join(&backup).join("Save0004.sav")).unwrap(), b"newer");
        assert!(!new.join("GameData").exists(), "the cache is rebuilt, not copied");
        assert!(old.join("settings.txt").is_file(), "the old folder is left as it is");
        assert!(new.join(MIGRATED_MARKER).is_file());

        // Once it has been done, nothing is copied again.
        std::fs::write(new.join("settings.txt"), "theme blue\n").unwrap();
        std::fs::remove_dir_all(new.join("backups")).unwrap();
        assert_eq!(migrate_old_data(&env, &new), Ok(false));
        assert_eq!(std::fs::read_to_string(new.join("settings.txt")).unwrap(), "theme blue\n");
        assert!(!new.join("backups").exists());

        // A new folder with settings of its own keeps them: only backups are added.
        std::fs::remove_file(new.join(MIGRATED_MARKER)).unwrap();
        assert_eq!(migrate_old_data(&env, &new), Ok(true));
        assert_eq!(std::fs::read_to_string(new.join("settings.txt")).unwrap(), "theme blue\n");
        assert!(new.join("backups").join(&backup).join("Save0004.sav").is_file());
        let _ = std::fs::remove_dir_all(&local);
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_own_files_stay_out_of_game_folders() {
        let env = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("LOCALAPPDATA", "X:\\Users\\me\\AppData\\Local")]);
        assert_eq!(app_data_dir(&env), PathBuf::from("X:\\Users\\me\\AppData\\Local\\VaultKey"));
        assert!(!is_in_game_folders(&env, &app_data_dir(&env)));
        let moved = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_DATA_DIR", "X:\\elsewhere")]);
        assert_eq!(app_data_dir(&moved), PathBuf::from("X:\\elsewhere"));
        for bad in ["X:\\Users\\me\\Documents\\My Games\\Borderlands 2\\WillowGame\\SaveData\\VaultKey", "X:\\Steam\\steamapps\\common\\Borderlands 2\\gk", "X:\\Lib\\SteamApps\\x"] {
            assert!(is_in_game_folders(&env, std::path::Path::new(bad)), "{bad}");
        }
        // A sandbox save folder given by GK_SAVEDATA_DIR counts as a save folder as well.
        let sandbox = env_of(&[("USERPROFILE", "X:\\Users\\me"), ("GK_SAVEDATA_DIR", "X:\\box\\SaveData")]);
        assert!(is_in_game_folders(&sandbox, std::path::Path::new("X:\\box\\SaveData\\inside")));
        assert!(!is_in_game_folders(&sandbox, std::path::Path::new("X:\\box\\VaultKey")));
    }
}
