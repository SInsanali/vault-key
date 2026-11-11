//! Backups: a copy of a save file taken before Vault Key replaces it.
//!
//! They live under the app's own data folder, never inside SaveData (Steam Cloud syncs that
//! folder). Layout:
//!
//! ```text
//! <backups>/<steam id>/<file name>/<stamp>[-n]/<file name>     the bytes, exactly
//! <backups>/<steam id>/<file name>/<stamp>[-n]/backup.txt      what it is a backup of
//! ```
//!
//! `<stamp>` is the UTC time, such as `20261109-031500Z`. A backup folder is only ever
//! created, never reused: a second backup in the same second gets `-2`, and so on.

use std::fs;
use std::path::{Path, PathBuf};

use savekit::SaveDocument;

use crate::error::{fail, CoreError, ErrorKind, Result};
use crate::guards;
use crate::slot::SlotId;
use crate::time::{self, Clock};

pub const BACKUPS_FOLDER_NAME: &str = "backups";
const INFO_FILE_NAME: &str = "backup.txt";
const INFO_HEADER: &str = "golden-key-backup 1";
/// More backups of one file in one second than this is a fault, not a workload.
const MAX_PER_SECOND: u32 = 10_000;

/// One backup. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// Identifies the backup: `<steam id>/<file name>/<stamp>[-n]`.
    pub id: String,
    /// The file this is a backup of.
    pub slot_id: SlotId,
    pub steam_id: String,
    pub file_name: String,
    pub slot: Option<u32>,
    /// When it was taken, in seconds since 1970-01-01 UTC.
    pub created_unix: u64,
    /// The same moment as text, such as `2026-11-09 03:15:00 UTC`.
    pub created_utc: String,
    /// Why it was taken, such as "Before saving 3 changes".
    pub reason: String,
    /// The character in the backup, if the bytes decode.
    pub character_name: Option<String>,
    pub level: Option<i64>,
    pub size: u64,
    /// Full path of the backed-up file, for display.
    pub path: String,
    /// SHA-1 of the bytes as recorded when the backup was taken (hex), if recorded.
    pub sha1: Option<String>,
}

/// The folder that holds every backup.
#[derive(Clone)]
pub struct BackupStore {
    root: PathBuf,
    clock: Clock,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One line of text: no line breaks or other control characters.
fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.trim().to_string()
}

/// Splits a backup folder name into its stamp and its sequence number (1 when there is no
/// `-n` suffix). `None` unless it is exactly a name this module creates.
fn parse_folder_name(name: &str) -> Option<(u64, u32)> {
    let (stamp, sequence) = match name.split_at_checked(16) {
        Some((stamp, "")) => (stamp, 1),
        Some((stamp, rest)) => {
            let digits = rest.strip_prefix('-')?;
            if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let sequence: u32 = digits.parse().ok()?;
            if sequence < 2 || digits.starts_with('0') {
                return None;
            }
            (stamp, sequence)
        }
        None => return None,
    };
    Some((time::parse_backup_stamp(stamp)?, sequence))
}

impl BackupStore {
    pub fn new(root: PathBuf, clock: Clock) -> BackupStore {
        BackupStore { root, clock }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Copies `data` (the current bytes of the file `slot_id`, found at `original_path`)
    /// into a new backup folder. Never overwrites: the folder is created fresh, and the
    /// copy is read back and compared before this returns.
    pub(crate) fn create(&self, slot_id: &SlotId, original_path: &Path, data: &[u8], reason: &str) -> Result<BackupInfo> {
        let problem =
            |what: &str, e: &dyn std::fmt::Display| CoreError::new(ErrorKind::BackupFailed, format!("Could not back up the save first, so nothing was changed ({what}: {e})."));
        let parent = self.root.join(slot_id.steam_id()).join(slot_id.file_name());
        fs::create_dir_all(&parent).map_err(|e| problem(&format!("cannot create {}", parent.display()), &e))?;

        let now = time::unix_seconds((self.clock)());
        let stamp = time::backup_stamp(now);
        let mut made: Option<(PathBuf, String)> = None;
        for sequence in 1..=MAX_PER_SECOND {
            let name = if sequence == 1 { stamp.clone() } else { format!("{stamp}-{sequence}") };
            let folder = parent.join(&name);
            // `create_dir` fails if the folder exists, so an earlier backup is never reused.
            match fs::create_dir(&folder) {
                Ok(()) => {
                    made = Some((folder, name));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(problem(&format!("cannot create {}", folder.display()), &e)),
            }
        }
        let Some((folder, name)) = made else {
            return Err(problem("too many backups in one second", &parent.display()));
        };

        let file = folder.join(slot_id.file_name());
        let info_file = folder.join(INFO_FILE_NAME);
        let doc = SaveDocument::from_file_bytes(data).ok();
        let info = BackupInfo {
            id: format!("{}/{}/{name}", slot_id.steam_id(), slot_id.file_name()),
            slot_id: slot_id.clone(),
            steam_id: slot_id.steam_id().to_string(),
            file_name: slot_id.file_name().to_string(),
            slot: slot_id.slot(),
            created_unix: now,
            created_utc: time::display_utc(now),
            reason: one_line(reason),
            character_name: doc.as_ref().map(|d| one_line(&d.name())),
            level: doc.as_ref().map(SaveDocument::level),
            size: data.len() as u64,
            path: file.display().to_string(),
            sha1: Some(hex(&savekit::checksums::sha1(data))),
        };

        let written = guards::write_new_file(&file, data)
            .map_err(|e| problem("cannot write the copy", &e))
            .and_then(|()| match fs::read(&file) {
                Ok(copy) if copy == data => Ok(()),
                Ok(_) => Err(problem("the copy read back differently", &file.display())),
                Err(e) => Err(problem("cannot read the copy back", &e)),
            })
            .and_then(|()| {
                guards::write_new_file(&info_file, info_text(&info, original_path).as_bytes())
                    .map_err(|e| problem("cannot write the backup's description", &e))
            });
        if let Err(e) = written {
            // Remove the half-made backup (only what was just created here).
            let _ = fs::remove_file(&file);
            let _ = fs::remove_file(&info_file);
            let _ = fs::remove_dir(&folder);
            return Err(e);
        }
        Ok(info)
    }

    /// Every backup, newest first. Folders that are not backups are skipped.
    pub fn list(&self) -> Vec<BackupInfo> {
        let mut found: Vec<(u64, u32, BackupInfo)> = Vec::new();
        for steam_id in sub_folders(&self.root) {
            for file_name in sub_folders(&self.root.join(&steam_id)) {
                let Ok(slot_id) = SlotId::new(&steam_id, &file_name) else { continue };
                let parent = self.root.join(&steam_id).join(&file_name);
                for name in sub_folders(&parent) {
                    if let Some((stamp, sequence, info)) = read_backup(&parent.join(&name), &slot_id, &name) {
                        found.push((stamp, sequence, info));
                    }
                }
            }
        }
        found.sort_by(|a, b| (b.0, b.1, &b.2.id).cmp(&(a.0, a.1, &a.2.id)));
        found.into_iter().map(|(_, _, info)| info).collect()
    }

    /// Backups of one file, newest first.
    pub fn list_for(&self, slot_id: &SlotId) -> Vec<BackupInfo> {
        self.list().into_iter().filter(|b| b.slot_id == *slot_id).collect()
    }

    /// The backup with this id.
    pub fn find(&self, id: &str) -> Result<BackupInfo> {
        let missing = || CoreError::new(ErrorKind::NoSuchBackup, format!("There is no backup \"{}\".", one_line(id)));
        let parts: Vec<&str> = id.split('/').collect();
        let [steam_id, file_name, name] = parts[..] else { return Err(missing()) };
        // Each part is checked, so an id can never point outside the backups folder.
        let slot_id = SlotId::new(steam_id, file_name).map_err(|_| missing())?;
        parse_folder_name(name).ok_or_else(missing)?;
        let folder = self.root.join(steam_id).join(file_name).join(name);
        read_backup(&folder, &slot_id, name).map(|(_, _, info)| info).ok_or_else(missing)
    }

    /// The bytes of a backup, checked against the SHA-1 recorded when it was taken.
    pub fn read(&self, info: &BackupInfo) -> Result<Vec<u8>> {
        let checked = self.find(&info.id)?;
        let bytes = fs::read(&checked.path).map_err(|e| {
            CoreError::new(ErrorKind::BadBackup, format!("Cannot read the backup {}: {e}", checked.path))
        })?;
        if let Some(recorded) = &checked.sha1 {
            if *recorded != hex(&savekit::checksums::sha1(&bytes)) {
                return fail(
                    ErrorKind::BadBackup,
                    format!("The backup {} has changed since it was taken, so it will not be used.", checked.path),
                );
            }
        }
        Ok(bytes)
    }
}

fn info_text(info: &BackupInfo, original_path: &Path) -> String {
    let mut out = format!("{INFO_HEADER}\n");
    out += &format!("slot {}\n", info.slot_id);
    out += &format!("original-path {}\n", one_line(&original_path.display().to_string()));
    out += &format!("created-unix {}\n", info.created_unix);
    out += &format!("created {}\n", info.created_utc);
    out += &format!("reason {}\n", info.reason);
    out += &format!("size {}\n", info.size);
    if let Some(sha1) = &info.sha1 {
        out += &format!("sha1 {sha1}\n");
    }
    if let Some(name) = &info.character_name {
        out += &format!("name {name}\n");
    }
    if let Some(level) = info.level {
        out += &format!("level {level}\n");
    }
    out
}

fn sub_folders(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Describes the backup in `folder`, if it is one: the folder name must be a stamp and the
/// backed-up file must be there. The description file adds detail but is not required.
fn read_backup(folder: &Path, slot_id: &SlotId, name: &str) -> Option<(u64, u32, BackupInfo)> {
    let (stamp, sequence) = parse_folder_name(name)?;
    let file = folder.join(slot_id.file_name());
    let meta = fs::metadata(&file).ok().filter(fs::Metadata::is_file)?;

    let text = fs::read(folder.join(INFO_FILE_NAME)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let value = |key: &str| -> Option<String> {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once(' ')?;
            (k == key).then(|| v.trim().to_string())
        })
    };
    let sha1 = value("sha1").filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    let info = BackupInfo {
        id: format!("{}/{}/{name}", slot_id.steam_id(), slot_id.file_name()),
        slot_id: slot_id.clone(),
        steam_id: slot_id.steam_id().to_string(),
        file_name: slot_id.file_name().to_string(),
        slot: slot_id.slot(),
        // The folder name is the record of when; the description file only repeats it.
        created_unix: stamp,
        created_utc: time::display_utc(stamp),
        reason: value("reason").unwrap_or_default(),
        character_name: value("name"),
        level: value("level").and_then(|v| v.parse().ok()),
        size: meta.len(),
        path: file.display().to_string(),
        sha1: sha1.map(|s| s.to_ascii_lowercase()),
    };
    Some((stamp, sequence, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{file_names, sample_bytes, TempDir, STEAM_ID};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    const NOW: u64 = 1_794_194_100; // 2026-11-09 03:15:00 UTC

    fn slot(file: &str) -> SlotId {
        SlotId::new(STEAM_ID, file).unwrap()
    }

    fn original() -> PathBuf {
        PathBuf::from("C:\\SaveData").join(STEAM_ID).join("Save0003.sav")
    }

    #[test]
    fn backups_never_overwrite_each_other_even_in_the_same_second() {
        let temp = TempDir::new();
        let store = BackupStore::new(temp.path().join("backups"), time::fixed_clock(NOW));
        let first = store.create(&slot("Save0003.sav"), &original(), b"first bytes", "Before saving 1 change").unwrap();
        let second = store.create(&slot("Save0003.sav"), &original(), b"second bytes", "Before saving 2 changes").unwrap();
        let third = store.create(&slot("Save0003.sav"), &original(), b"third bytes", "Before restoring").unwrap();
        assert_eq!(first.id, format!("{STEAM_ID}/Save0003.sav/20261109-031500Z"));
        assert_eq!(second.id, format!("{STEAM_ID}/Save0003.sav/20261109-031500Z-2"));
        assert_eq!(third.id, format!("{STEAM_ID}/Save0003.sav/20261109-031500Z-3"));
        assert_eq!(fs::read(&first.path).unwrap(), b"first bytes");
        assert_eq!(fs::read(&second.path).unwrap(), b"second bytes");
        assert_eq!(fs::read(&third.path).unwrap(), b"third bytes");
        assert_eq!(store.read(&first).unwrap(), b"first bytes");

        // Newest first: within one second, the later sequence number is the newer one.
        let ids: Vec<String> = store.list().into_iter().map(|b| b.id).collect();
        assert_eq!(ids, [third.id.clone(), second.id.clone(), first.id.clone()]);
        assert_eq!(store.find(&second.id).unwrap(), second);
    }

    #[test]
    fn lists_what_each_backup_is_of_and_when() {
        let temp = TempDir::new();
        let now = Arc::new(AtomicU64::new(NOW));
        let ticking = now.clone();
        let clock: Clock = Arc::new(move || {
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(ticking.fetch_add(60, Ordering::SeqCst))
        });
        let store = BackupStore::new(temp.path().join("backups"), clock);
        let save = sample_bytes(3);
        let a = store.create(&slot("Save0003.sav"), &original(), &save, "Before saving 1 change").unwrap();
        let b = store.create(&slot("Save0004.sav"), &original(), b"not a save", "Before restoring\nline two").unwrap();
        let c = store.create(&slot("Save0003.sav"), &original(), &save, "Before saving 2 changes").unwrap();

        let list = store.list();
        assert_eq!(list, [c.clone(), b.clone(), a.clone()]);
        assert_eq!(a.created_utc, "2026-11-09 03:15:00 UTC");
        assert_eq!(c.created_unix, NOW + 120);
        assert_eq!((a.character_name.as_deref(), a.level, a.slot), (Some("Gaige"), Some(5), Some(3)));
        assert_eq!((b.character_name.as_deref(), b.level), (None, None));
        assert_eq!(b.reason, "Before restoring line two");
        assert_eq!(a.size, save.len() as u64);
        assert_eq!(a.slot_id, slot("Save0003.sav"));

        let of_three: Vec<String> = store.list_for(&slot("Save0003.sav")).into_iter().map(|b| b.id).collect();
        assert_eq!(of_three, [c.id, a.id]);
        assert!(store.list_for(&slot("Save0009.sav")).is_empty());
    }

    #[test]
    fn a_changed_or_missing_backup_is_not_used() {
        let temp = TempDir::new();
        let store = BackupStore::new(temp.path().join("backups"), time::fixed_clock(NOW));
        let info = store.create(&slot("Save0003.sav"), &original(), b"good bytes", "why").unwrap();
        fs::write(&info.path, b"evil bytes").unwrap();
        assert_eq!(store.read(&info).unwrap_err().kind, ErrorKind::BadBackup);
        fs::remove_file(&info.path).unwrap();
        assert_eq!(store.read(&info).unwrap_err().kind, ErrorKind::NoSuchBackup);
        assert!(store.list().is_empty());
    }

    #[test]
    fn ids_cannot_point_outside_the_backups_folder() {
        let temp = TempDir::new();
        let store = BackupStore::new(temp.path().join("backups"), time::fixed_clock(NOW));
        store.create(&slot("Save0003.sav"), &original(), b"bytes", "why").unwrap();
        fs::write(temp.path().join("Save0003.sav"), b"outside").unwrap();
        for id in [
            "",
            "20261109-031500Z",
            "../Save0003.sav/20261109-031500Z",
            &format!("{STEAM_ID}/../20261109-031500Z"),
            &format!("{STEAM_ID}/Save0003.sav/.."),
            &format!("{STEAM_ID}/Save0003.sav/20261109-031500Z/.."),
            &format!("{STEAM_ID}/Save0003.sav/20261109-031500Z-1"),
            &format!("{STEAM_ID}/Save0003.sav/20261109-031500Z-02"),
            &format!("{STEAM_ID}/Save0003.sav/20261109-031501Z"),
            &format!("{STEAM_ID}\\Save0003.sav\\20261109-031500Z"),
        ] {
            assert_eq!(store.find(id).unwrap_err().kind, ErrorKind::NoSuchBackup, "{id}");
        }
    }

    #[test]
    fn a_backup_without_its_description_is_still_listed() {
        let temp = TempDir::new();
        let store = BackupStore::new(temp.path().join("backups"), time::fixed_clock(NOW));
        let info = store.create(&slot("Save0003.sav"), &original(), b"bytes", "why").unwrap();
        let folder = Path::new(&info.path).parent().unwrap().to_path_buf();
        fs::remove_file(folder.join(INFO_FILE_NAME)).unwrap();
        // Unrelated folders and files are skipped.
        fs::create_dir_all(folder.parent().unwrap().join("not-a-backup")).unwrap();
        fs::create_dir_all(store.root().join("misc")).unwrap();
        fs::write(store.root().join("readme.txt"), b"x").unwrap();
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].created_unix, list[0].reason.as_str()), (info.id.as_str(), NOW, ""));
        assert_eq!(store.read(&list[0]).unwrap(), b"bytes");
    }

    #[test]
    fn a_backup_folder_that_cannot_be_made_is_an_error_and_leaves_nothing_behind() {
        let temp = TempDir::new();
        // A plain file where the backups folder should be.
        let root = temp.path().join("backups");
        fs::write(&root, b"in the way").unwrap();
        let store = BackupStore::new(root.clone(), time::fixed_clock(NOW));
        let error = store.create(&slot("Save0003.sav"), &original(), b"bytes", "why").unwrap_err();
        assert_eq!(error.kind, ErrorKind::BackupFailed);
        assert!(error.message.contains("nothing was changed"));
        assert_eq!(fs::read(&root).unwrap(), b"in the way");
        assert_eq!(file_names(temp.path()), ["backups"]);
        assert!(store.list().is_empty());
    }

    #[test]
    fn folder_names() {
        assert_eq!(parse_folder_name("20261109-031500Z"), Some((NOW, 1)));
        assert_eq!(parse_folder_name("20261109-031500Z-2"), Some((NOW, 2)));
        assert_eq!(parse_folder_name("20261109-031500Z-10000"), Some((NOW, 10_000)));
        for bad in ["", "20261109-031500Z-", "20261109-031500Z-1", "20261109-031500Z-0", "20261109-031500Z2", "20261109-031500Z-2x", "20261109", "é0261109-031500Z"] {
            assert_eq!(parse_folder_name(bad), None, "{bad}");
        }
    }
}
