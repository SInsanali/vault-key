//! Safety guards for anything that touches the file system on the user's behalf.
//!
//! These exist to protect real characters: output never lands in the game's save folder
//! except through `install`, `install` never overwrites, and nothing is installed while the
//! game is running or when two saves could end up sharing a save-game ID.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// A refusal or failure. The message is the one-line reason shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GkError(pub String);

impl fmt::Display for GkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GkError {}

pub fn refuse<T>(message: impl Into<String>) -> Result<T, GkError> {
    Err(GkError(message.into()))
}

/// Environment variable that points the tool at a different SaveData folder (safe testing).
pub const SAVEDATA_ENV: &str = "GK_SAVEDATA_DIR";

/// Name of the game's executable on Windows.
pub const GAME_EXE: &str = "Borderlands2.exe";

const SAVEDATA_TAIL: [&str; 4] = ["My Games", "Borderlands 2", "WillowGame", "SaveData"];

/// Candidate locations of the real SaveData folder, most likely first.
///
/// The Documents folder is normally `%USERPROFILE%\Documents`, but OneDrive can redirect it.
pub fn real_save_data_candidates(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut docs: Vec<PathBuf> = Vec::new();
    if let Some(profile) = env("USERPROFILE") {
        docs.push(Path::new(&profile).join("Documents"));
    }
    for var in ["OneDrive", "OneDriveConsumer"] {
        if let Some(one) = env(var) {
            docs.push(Path::new(&one).join("Documents"));
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for d in docs {
        let mut p = d;
        for part in SAVEDATA_TAIL {
            p.push(part);
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Every real SaveData folder that exists on this machine.
pub fn real_save_data_dirs() -> Vec<PathBuf> {
    real_save_data_candidates(&|k| std::env::var_os(k)).into_iter().filter(|p| p.is_dir()).collect()
}

/// The SaveData root to work in: the override if set, otherwise the first real folder that exists.
pub fn save_data_root(env: &dyn Fn(&str) -> Option<OsString>) -> Result<PathBuf, GkError> {
    if let Some(o) = env(SAVEDATA_ENV) {
        if !o.is_empty() {
            return Ok(PathBuf::from(o));
        }
    }
    let candidates = real_save_data_candidates(env);
    match candidates.iter().find(|p| p.is_dir()) {
        Some(p) => Ok(p.clone()),
        None => refuse(format!(
            "cannot find the game's save folder (looked in: {})",
            candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The physical location of `path`: links are resolved for the part that exists, and the
/// rest (which may not exist yet) is appended with `.` and `..` applied.
pub fn physical_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // Find the longest existing ancestor, resolving it through any links.
    let mut existing = absolute.clone();
    let base = loop {
        if let Ok(real) = fs::canonicalize(&existing) {
            break real;
        }
        match existing.parent() {
            Some(parent) => existing = parent.to_path_buf(),
            None => break existing.clone(),
        }
    };
    // Append what does not exist yet, applying `.` and `..` as plain path steps.
    let consumed = existing.components().count();
    let mut out = base;
    for comp in absolute.components().skip(consumed) {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn folded(path: &Path) -> Vec<String> {
    physical_path(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// True when `path` is `root` or lies anywhere under it. Link-proof and case-insensitive
/// (Windows file names are case-insensitive; on a case-sensitive volume this over-refuses,
/// which is the safe side).
pub fn is_inside(path: &Path, root: &Path) -> bool {
    let p = folded(path);
    let r = folded(root);
    p.len() >= r.len() && p[..r.len()] == r[..]
}

/// Output files must not already exist and must not lie inside any of the SaveData folders
/// in `roots` (the working root and every real one).
pub fn check_output(out: &Path, roots: &[PathBuf]) -> Result<(), GkError> {
    if fs::symlink_metadata(out).is_ok() {
        return refuse(format!("refused: output file already exists: {}", out.display()));
    }
    if roots.iter().any(|r| is_inside(out, r)) {
        return refuse("refused: output must not be inside the game's SaveData folder (use 'install')");
    }
    Ok(())
}

/// The single all-digit Steam-ID folder inside `root`.
pub fn steam_dir(root: &Path) -> Result<PathBuf, GkError> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return refuse(format!("refused: cannot read SaveData folder: {}", root.display())),
    };
    let mut ids: BTreeSet<PathBuf> = BTreeSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) && entry.path().is_dir() {
            ids.insert(entry.path());
        }
    }
    if ids.len() != 1 {
        return refuse(format!(
            "refused: expected exactly one Steam-ID folder in {}, found {}",
            root.display(),
            ids.len()
        ));
    }
    Ok(ids.into_iter().next().expect("one entry"))
}

/// Lowest and highest slot `install` will write. Slot 1 is never written. Above 9 the game's
/// file names are hexadecimal, which is unverified, so those are refused for now.
pub const MIN_SLOT: u32 = 2;
pub const MAX_SLOT: u32 = 9;

pub fn slot_number(s: &str) -> Result<u32, GkError> {
    let n: u32 = match s.trim().parse() {
        Ok(n) => n,
        Err(_) => return refuse(slot_range_message()),
    };
    if n == 1 {
        return refuse("refused: slot 1 is the main character's slot and is never written");
    }
    if !(MIN_SLOT..=MAX_SLOT).contains(&n) {
        return refuse(slot_range_message());
    }
    Ok(n)
}

fn slot_range_message() -> String {
    format!(
        "refused: slot must be a number {MIN_SLOT}...{MAX_SLOT} (higher slots use hexadecimal file names, which are not supported yet)"
    )
}

pub fn slot_file_name(slot: u32) -> String {
    format!("Save{slot:04}.sav")
}

/// True for the game's save files, in any letter case: `Save*.sav`.
pub fn is_save_file_name(name: &str) -> bool {
    let l = name.to_lowercase();
    l.starts_with("save") && l.ends_with(".sav")
}

/// The slot a save file name stands for: `Save` + four hexadecimal digits + `.sav`, in any
/// letter case. Anything else (a copy, a backup, another file) is `None`.
///
/// Decimal and hexadecimal agree for slots 1 to 9; above that the hexadecimal reading is
/// unverified, so those slots are listed but never written.
pub fn slot_from_file_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let digits = lower.strip_prefix("save")?.strip_suffix(".sav")?;
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// True when `data` is byte-identical to a `Save0001.sav` in any real SaveData folder.
pub fn is_real_save_0001(data: &[u8], real_roots: &[PathBuf]) -> bool {
    for root in real_roots {
        let Ok(entries) = fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            if let Ok(real) = fs::read(entry.path().join("Save0001.sav")) {
                if real == data {
                    return true;
                }
            }
        }
    }
    false
}

/// File name of an existing save in `dir` whose save-game ID equals `id`, if any.
///
/// Fails closed: an error if the folder cannot be listed, or if any save file cannot be read
/// or decoded, because that file might hold the same ID. `save_game_id` decodes a save's bytes.
pub fn slot_using_save_game_id(
    id: u64,
    dir: &Path,
    save_game_id: &dyn Fn(&[u8]) -> Option<u64>,
) -> Result<Option<String>, GkError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return refuse(format!("refused: cannot list {} to check save-game IDs", dir.display())),
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => names.push(e.file_name().to_string_lossy().into_owned()),
            Err(_) => return refuse(format!("refused: cannot list {} to check save-game IDs", dir.display())),
        }
    }
    names.sort();
    let mut found = None;
    for name in names {
        if !is_save_file_name(&name) {
            continue;
        }
        let decoded = fs::read(dir.join(&name)).ok().and_then(|d| save_game_id(&d));
        let Some(existing) = decoded else {
            return refuse(format!(
                "refused: cannot read or decode {name} in the save folder, so its save-game ID is unknown; fix or move it first"
            ));
        };
        if existing == id && found.is_none() {
            found = Some(name);
        }
    }
    Ok(found)
}

/// True when the game's executable is among the running process image names.
pub fn game_is_running(process_names: &[String]) -> bool {
    process_names.iter().any(|n| {
        let file = n.rsplit(['\\', '/']).next().unwrap_or(n);
        file.eq_ignore_ascii_case(GAME_EXE)
    })
}

/// Image names of the running processes, from `tasklist`.
///
/// An error when the list cannot be obtained: callers must treat that as "cannot prove the
/// game is closed" and refuse, not as "not running".
pub fn live_process_names() -> Result<Vec<String>, GkError> {
    let mut command = std::process::Command::new("tasklist");
    command.args(["/FO", "CSV", "/NH"]);
    #[cfg(windows)]
    {
        // Without this, a windowed app flashes a console window every time it asks.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| GkError(format!("refused: cannot list running processes ({e}), so cannot tell whether the game is closed")))?;
    if !output.status.success() {
        return refuse("refused: cannot list running processes, so cannot tell whether the game is closed");
    }
    let names = parse_tasklist_csv(&String::from_utf8_lossy(&output.stdout));
    if names.is_empty() {
        return refuse("refused: the process list came back empty, so cannot tell whether the game is closed");
    }
    Ok(names)
}

/// First CSV column of each `tasklist /FO CSV /NH` line: the image name.
pub fn parse_tasklist_csv(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('"')?;
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

/// Write `data` to a new file, failing if anything is already there.
pub fn write_new_file(path: &Path, data: &[u8]) -> Result<(), GkError> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| GkError(format!("cannot write {}: {e}", path.display())))?;
    f.write_all(data)
        .and_then(|_| f.sync_all())
        .map_err(|e| GkError(format!("cannot write {}: {e}", path.display())))
}

/// Place `data` at `target` atomically, never replacing an existing file: write a temp file in
/// the same folder, then hard-link it to the target name (which fails if the name is taken).
pub fn place_exclusively(target: &Path, data: &[u8], unique: &str) -> Result<(), GkError> {
    let dir = target.parent().ok_or_else(|| GkError(format!("bad target path: {}", target.display())))?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".gk-install-{unique}.tmp"));
    write_new_file(&tmp, data)?;
    let linked = fs::hard_link(&tmp, target);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) => refuse(format!("refused: could not place {name} ({e})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A fresh, empty temp folder that is removed when dropped.
    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub(crate) fn new() -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p = std::env::temp_dir().join(format!("gk-test-{}-{nanos}-{n}", std::process::id()));
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub(crate) fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn override_wins_and_candidates_cover_onedrive() {
        let env = env_of(&[("GK_SAVEDATA_DIR", "X:\\sandbox"), ("USERPROFILE", "C:\\Users\\me")]);
        assert_eq!(save_data_root(&env).unwrap(), PathBuf::from("X:\\sandbox"));

        let env = env_of(&[("USERPROFILE", "C:\\Users\\me"), ("OneDrive", "C:\\Users\\me\\OneDrive")]);
        let c = real_save_data_candidates(&env);
        assert_eq!(c.len(), 2);
        assert!(c[0].ends_with("Documents/My Games/Borderlands 2/WillowGame/SaveData"));
        assert!(c[1].to_string_lossy().contains("OneDrive"));
    }

    #[test]
    fn missing_save_folder_is_an_error_not_a_guess() {
        let env = env_of(&[("USERPROFILE", "Q:\\no-such-profile-gk")]);
        assert!(save_data_root(&env).is_err());
    }

    #[test]
    fn is_inside_handles_case_dots_and_missing_tails() {
        let t = TempDir::new();
        let root = t.path().join("SaveData");
        fs::create_dir_all(root.join("123")).unwrap();
        assert!(is_inside(&root, &root));
        assert!(is_inside(&root.join("123").join("Save0002.sav"), &root));
        assert!(is_inside(&root.join("123").join("new").join("deep.sav"), &root));
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
        assert!(is_inside(&upper.join("123").join("x.sav"), &root));
        // `..` that escapes, including through a folder that does not exist yet.
        assert!(!is_inside(&root.join("..").join("out.sav"), &root));
        assert!(!is_inside(&root.join("nope").join("..").join("..").join("out.sav"), &root));
        assert!(is_inside(&root.join("nope").join("..").join("in.sav"), &root));
        // A sibling whose name merely starts with the root's name is outside.
        assert!(!is_inside(&t.path().join("SaveDataBackup").join("x.sav"), &root));
        assert!(!is_inside(&t.path().join("elsewhere.sav"), &root));
    }

    #[test]
    fn is_inside_sees_through_a_junction() {
        let t = TempDir::new();
        let root = t.path().join("SaveData");
        fs::create_dir_all(&root).unwrap();
        let link = t.path().join("innocent");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !made {
            eprintln!("skipped: could not create a junction here");
            return;
        }
        assert!(is_inside(&link.join("Save0005.sav"), &root));
        assert!(check_output(&link.join("Save0005.sav"), std::slice::from_ref(&root)).is_err());
        let _ = fs::remove_dir(&link);
    }

    #[test]
    fn check_output_refuses_existing_and_save_folders() {
        let t = TempDir::new();
        let root = t.path().join("SaveData");
        let real = t.path().join("RealSaveData");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&real).unwrap();
        let roots = vec![root.clone(), real.clone()];
        let existing = t.path().join("there.sav");
        fs::write(&existing, b"x").unwrap();
        assert!(check_output(&existing, &roots).unwrap_err().0.contains("already exists"));
        assert!(check_output(&root.join("a.sav"), &roots).is_err());
        assert!(check_output(&real.join("1").join("a.sav"), &roots).is_err());
        assert!(check_output(&t.path().join("fine.sav"), &roots).is_ok());
        // With no known save folder, only the "already exists" rule applies.
        assert!(check_output(&t.path().join("fine.sav"), &[]).is_ok());
        assert!(check_output(&existing, &[]).is_err());
    }

    #[test]
    fn steam_dir_needs_exactly_one_numeric_folder() {
        let t = TempDir::new();
        assert!(steam_dir(t.path()).is_err());
        fs::create_dir(t.path().join("76561198000000000")).unwrap();
        fs::create_dir(t.path().join("backup-20261107")).unwrap();
        fs::write(t.path().join("12345"), b"a file, not a folder").unwrap();
        assert_eq!(steam_dir(t.path()).unwrap(), t.path().join("76561198000000000"));
        fs::create_dir(t.path().join("111")).unwrap();
        assert!(steam_dir(t.path()).unwrap_err().0.contains("found 2"));
        assert!(steam_dir(&t.path().join("missing")).is_err());
    }

    #[test]
    fn slot_rules() {
        assert!(slot_number("1").unwrap_err().0.contains("slot 1"));
        for bad in ["0", "10", "16", "-2", "a", "", "2.5", "0x3"] {
            assert!(slot_number(bad).is_err(), "{bad}");
        }
        for ok in 2..=9u32 {
            assert_eq!(slot_number(&ok.to_string()).unwrap(), ok);
        }
        assert_eq!(slot_file_name(3), "Save0003.sav");
        assert!(is_save_file_name("SAVE0003.SAV") && is_save_file_name("save000a.sav"));
        assert!(!is_save_file_name("profile.bin") && !is_save_file_name("Save0003.sav.bak"));
    }

    #[test]
    fn slot_from_file_name_is_strict() {
        assert_eq!(slot_from_file_name("Save0004.sav"), Some(4));
        assert_eq!(slot_from_file_name("save000a.SAV"), Some(10));
        assert_eq!(slot_from_file_name("SAVE0001.SAV"), Some(1));
        for bad in ["Save0001.sav.bak", "Player.wsg", "Save1.sav", "Save00011.sav", "Save000g.sav", "Save+001.sav", "", "xSave0001.sav"] {
            assert_eq!(slot_from_file_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn id_scan_finds_matches_and_fails_closed() {
        let t = TempDir::new();
        fs::write(t.path().join("Save0001.sav"), [1u8]).unwrap();
        fs::write(t.path().join("SAVE0003.SAV"), [3u8]).unwrap();
        fs::write(t.path().join("profile.bin"), [9u8, 9]).unwrap();
        let id_of = |d: &[u8]| if d.len() == 1 { Some(d[0] as u64) } else { None };
        assert_eq!(slot_using_save_game_id(3, t.path(), &id_of).unwrap(), Some("SAVE0003.SAV".to_string()));
        assert_eq!(slot_using_save_game_id(4, t.path(), &id_of).unwrap(), None);
        // An undecodable save might hold the ID, so the scan must refuse rather than say "free".
        fs::write(t.path().join("Save0005.sav"), [5u8, 5]).unwrap();
        assert!(slot_using_save_game_id(4, t.path(), &id_of).unwrap_err().0.contains("Save0005.sav"));
        assert!(slot_using_save_game_id(4, &t.path().join("missing"), &id_of).is_err());
    }

    #[test]
    fn real_save_0001_match_is_byte_exact() {
        let t = TempDir::new();
        let real = t.path().join("SaveData");
        fs::create_dir_all(real.join("111")).unwrap();
        fs::write(real.join("111").join("Save0001.sav"), b"main character").unwrap();
        let roots = vec![real];
        assert!(is_real_save_0001(b"main character", &roots));
        assert!(!is_real_save_0001(b"main character!", &roots));
        assert!(!is_real_save_0001(b"main character", &[t.path().join("missing")]));
    }

    #[test]
    fn game_running_detection() {
        let csv = "\"System Idle Process\",\"0\",\"Services\",\"0\",\"8 K\"\r\n\"Borderlands2.exe\",\"4242\",\"Console\",\"1\",\"900,000 K\"\r\n";
        let names = parse_tasklist_csv(csv);
        assert_eq!(names, vec!["System Idle Process", "Borderlands2.exe"]);
        assert!(game_is_running(&names));
        assert!(game_is_running(&["C:\\Games\\BORDERLANDS2.EXE".to_string()]));
        assert!(!game_is_running(&["Borderlands2Launcher.exe".to_string(), "PowerToys.PowerLauncher.exe".to_string()]));
        assert!(!game_is_running(&[]));
    }

    #[test]
    fn live_process_list_works_here() {
        let names = live_process_names().unwrap();
        assert!(names.len() > 5);
    }

    #[test]
    fn place_exclusively_never_overwrites() {
        let t = TempDir::new();
        let target = t.path().join("Save0003.sav");
        place_exclusively(&target, b"first", "a").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        assert!(place_exclusively(&target, b"second", "b").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"first");
        let leftovers: Vec<_> = fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(leftovers.len(), 1, "temp files must be cleaned up: {leftovers:?}");
        assert!(write_new_file(&target, b"third").is_err());
    }
}
