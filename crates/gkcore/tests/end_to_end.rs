//! End to end on real saves, through the public API only (as a front end would use it).
//!
//! Runs on every `.sav` in `fixtures/local` at the repository root, each copied into
//! a fresh temp SaveData folder, and passes without doing anything when there is none.
//! Nothing here touches a real user folder: the save folder, the settings and the backups
//! are all under a temp folder, and the process list is a fake.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use gkcore::{
    Core, CoreConfig, Currency, Edit, ErrorKind, FallbackGearClassifier, SaveDocument, SlotId, WritableBecause,
    WriteAccess, BACKPACK_FULL_MESSAGE,
};

const STEAM_ID: &str = "76561190000000000";
const NOW: u64 = 1_794_194_100; // 2026-11-09 03:15:00 UTC

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("gkcore-e2e-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local");
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "sav")).collect())
        .unwrap_or_default();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let bytes = fs::read(&p).unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", p.display()));
            (name, bytes)
        })
        .collect()
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// Says which fixtures the test below ran on (shown with `--nocapture`).
#[test]
fn report_fixtures() {
    let found = fixtures();
    eprintln!("gkcore end-to-end ran on {} save(s) from fixtures/local", found.len());
    for (name, bytes) in &found {
        eprintln!("  {name}: {} bytes", bytes.len());
    }
}

#[test]
fn open_stage_save_as_new_edit_in_place_and_restore() {
    for (name, fixture) in fixtures() {
        let temp = TempDir::new();
        let save_root = temp.0.join("SaveData");
        let folder = save_root.join(STEAM_ID);
        fs::create_dir_all(&folder).unwrap();
        // The fixture plays the part of an existing character the app did not create.
        fs::write(folder.join("Save0001.sav"), &fixture).unwrap();
        let before = SaveDocument::from_file_bytes(&fixture).unwrap_or_else(|e| panic!("{name}: {e}"));

        let game_running = Arc::new(AtomicBool::new(false));
        let running = game_running.clone();
        let mut core = Core::new(CoreConfig {
            save_data_root: save_root.clone(),
            data_dir: temp.0.join("VaultKey"),
            other_save_roots: Vec::new(),
            processes: Arc::new(move || {
                let mut names = vec!["explorer.exe".to_string()];
                if running.load(Ordering::SeqCst) {
                    names.push("Borderlands2.exe".to_string());
                }
                Ok(names)
            }),
            clock: gkcore::fixed_clock(NOW),
            classifier: Arc::new(FallbackGearClassifier),
        })
        .unwrap();

        // The character list.
        let list = core.list_characters().unwrap();
        assert_eq!(list.len(), 1, "{name}");
        let entry = &list[0];
        assert!(entry.is_readable() && !entry.writable, "{name}");
        assert_eq!((entry.slot, entry.file_name.as_str()), (Some(1), "Save0001.sav"), "{name}");
        assert_eq!((entry.name.clone(), entry.level, entry.save_game_id), (before.name(), before.level(), before.save_game_id()), "{name}");
        assert_eq!(entry.class_path, before.class_path(), "{name}");
        assert_eq!(entry.size, fixture.len() as u64, "{name}");

        // Open and stage: level, money, backpack 39, and a duplicated weapon.
        let mut session = core.open(&entry.slot_id).unwrap();
        assert_eq!(session.original_bytes(), fixture, "{name}");
        let weapon = session.items().into_iter().find(|row| row.is_weapon && row.can_duplicate);
        if let Some(weapon) = &weapon {
            if before.backpack_used() as i64 >= before.backpack_slots() {
                // A fresh character's backpack is already full: the copy is blocked until
                // there is room.
                let error = session.duplicate_item(weapon.entry_ref).unwrap_err();
                assert_eq!((error.kind, error.message.as_str()), (ErrorKind::BackpackFull, BACKPACK_FULL_MESSAGE), "{name}");
                assert!(!session.has_changes(), "{name}");
            }
        }
        session.stage(Edit::Level(30)).unwrap();
        session.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        session.stage(Edit::BackpackSlots(39)).unwrap();
        let mut staged = 3;
        if let Some(weapon) = &weapon {
            let outcome = session.duplicate_item(weapon.entry_ref).unwrap();
            assert_eq!(outcome.new_entries.len(), 1, "{name}");
            staged += 1;
            // Staged, and removable before saving: take it out and put it back.
            session.remove_change(outcome.change_id.unwrap()).unwrap();
            assert_eq!(session.pending_count(), 3, "{name}");
            session.duplicate_item(weapon.entry_ref).unwrap();
        }
        assert_eq!(session.pending_count(), staged, "{name}");
        eprintln!("  {name}: staged {staged} changes ({})", if weapon.is_some() { "a weapon was duplicated" } else { "no weapon to duplicate" });
        let lines: Vec<String> = session.change_lines().into_iter().map(|line| line.text).collect();
        assert_eq!(lines[0], format!("Level {} → 30", before.level()), "{name}");
        assert_eq!(lines.len(), staged, "{name}");
        let weapons_before = before.inventory().iter().filter(|e| e.is_weapon).count();

        // The existing file is read-only: saving in place is refused and changes nothing.
        let error = core.save_in_place(&mut session).unwrap_err();
        assert_eq!(error.kind, ErrorKind::ReadOnly, "{name}");
        assert_eq!(fs::read(folder.join("Save0001.sav")).unwrap(), fixture, "{name}");

        // Not while the game runs.
        game_running.store(true, Ordering::SeqCst);
        assert_eq!(core.save_as_new(&session, 3).unwrap_err().kind, ErrorKind::GameRunning, "{name}");
        game_running.store(false, Ordering::SeqCst);
        assert_eq!(file_names(&folder), ["Save0001.sav"], "{name}");

        // Save as a new character in slot 3.
        let report = core.save_as_new(&session, 3).unwrap();
        assert!(report.created_new && report.warning.is_none(), "{name}");
        let new_slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        assert_eq!(report.slot_id, new_slot, "{name}");
        assert_eq!(fs::read(folder.join("Save0001.sav")).unwrap(), fixture, "{name}: the source is untouched");
        assert_eq!(core.access(&new_slot), WriteAccess::Writable(WritableBecause::CreatedByApp), "{name}");
        let list = core.list_characters().unwrap();
        assert_eq!(list.iter().map(|c| c.writable).collect::<Vec<_>>(), [false, true], "{name}");
        assert_eq!((list[1].level, list[1].save_game_id), (30, 3), "{name}");

        // Reopen the new character.
        let mut session = core.open(&new_slot).unwrap();
        let snapshot = session.snapshot();
        assert_eq!((snapshot.level, snapshot.money, snapshot.backpack_slots, snapshot.save_game_id), (30, 123_456, 39, 3), "{name}");
        assert_eq!(snapshot.name, before.name(), "{name}");
        let weapons_now = session.items().iter().filter(|row| row.is_weapon).count();
        assert_eq!(weapons_now, weapons_before + usize::from(weapon.is_some()), "{name}");
        assert!(!session.has_changes(), "{name}");

        // Stage another edit and save in place.
        let pre_edit = fs::read(folder.join("Save0003.sav")).unwrap();
        session.stage(Edit::Currency(Currency::Eridium, 50)).unwrap();
        session.stage(Edit::UnlockTvhm).unwrap();
        let report = core.save_in_place(&mut session).unwrap();
        let backup = report.backup.unwrap();
        assert_eq!(fs::read(&backup.path).unwrap(), pre_edit, "{name}");
        assert!(Path::new(&backup.path).starts_with(temp.0.join("VaultKey").join("backups")), "{name}");
        let edited = fs::read(folder.join("Save0003.sav")).unwrap();
        assert_ne!(edited, pre_edit, "{name}");
        let reread = SaveDocument::from_file_bytes(&edited).unwrap();
        assert_eq!((reread.eridium(), reread.level(), reread.money()), (50, 30, 123_456), "{name}");
        assert!(reread.playthroughs_unlocked() >= 1, "{name}");
        assert!(!session.has_changes(), "{name}");

        // Restore the backup: the file equals the pre-edit bytes.
        let report = core.restore_backup(&backup.id).unwrap();
        assert_eq!(fs::read(folder.join("Save0003.sav")).unwrap(), pre_edit, "{name}");
        // The restore itself was backed up, in the same second, without touching the first backup.
        let before_restore = report.backup.unwrap();
        assert_ne!(before_restore.id, backup.id, "{name}");
        assert_eq!(fs::read(&before_restore.path).unwrap(), edited, "{name}");
        assert_eq!(fs::read(&backup.path).unwrap(), pre_edit, "{name}");
        assert_eq!(core.list_backups().len(), 2, "{name}");
        assert!(core.list_backups().iter().all(|b| b.slot_id == new_slot && b.created_utc == "2026-11-09 03:15:00 UTC"), "{name}");

        // Nothing else was left in the save folder, and the source never changed.
        assert_eq!(file_names(&folder), ["Save0001.sav", "Save0003.sav"], "{name}");
        assert_eq!(file_names(&save_root), [STEAM_ID], "{name}");
        assert_eq!(fs::read(folder.join("Save0001.sav")).unwrap(), fixture, "{name}");
        assert_eq!(file_names(&temp.0.join("VaultKey")), ["backups", "settings.txt"], "{name}");
    }
}
