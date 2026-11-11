//! Shared test helpers: temp folders, synthetic saves and a fake process list.
//!
//! Nothing here touches a real user folder. Every folder a test uses is a fresh one under
//! the system temp folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use savekit::{field, EntryRef, ProtoValue, SaveDocument};

use crate::guards::GkError;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh, empty temp folder that is removed when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("gkcore-test-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A test may have left a read-only file behind.
        let _ = clear_read_only(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clear_read_only(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let meta = entry.metadata()?;
        if meta.is_dir() {
            let _ = clear_read_only(&path);
        } else if meta.permissions().readonly() {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&path, permissions);
        }
    }
    Ok(())
}

pub(crate) const STEAM_ID: &str = "76561190000000000";

/// A weapon with a real manufacturer (gear), from the save format's test vectors.
pub(crate) const GEAR_WEAPON_CODE: &str = "BL2(hwAAAAC8CQCBxwJABBEI0MJghdENIwHG//////////97w/iGMQzj)";
/// A second, different weapon (level 50).
pub(crate) const WEAPON_CODE: &str = "BL2(h0Hd1Z+jY/s2Qy++Zu8Ba9qXoOmjwJ6NhrlsOmhNMX+oJo5CfQns)";
/// A level-1 gear item.
pub(crate) const ITEM_CODE: &str = "BL2(B2vuv4tz1zSQCf2pqLJCS5XD/tKN4FXpjRJLnn1v85U=)";
/// An intro ECHO: a story item with no manufacturer, so not gear.
pub(crate) const ECHO_CODE: &str = "BL2(BwAAAABsWgADYgaQ4/8AwA==)";

pub(crate) const CLASS_PATH: &str = "GD_Tulip_Mechromancer.Character.CharClass_Mechromancer";

/// A small but complete character: Gaige, level 5, backpack 12 with one gear weapon
/// (`54:0`), one gear item (`53:0`) and one story item (`53:1`).
pub(crate) fn sample_doc(save_game_id: i64) -> SaveDocument {
    let mut doc = SaveDocument::default();
    doc.message.set_single(field::CLASS, ProtoValue::Bytes(CLASS_PATH.as_bytes().to_vec()));
    doc.set_level(5).unwrap();
    doc.set_name("Gaige").unwrap();
    doc.set_save_game_id(save_game_id).unwrap();
    doc.set_backpack_slots(12).unwrap();
    doc.set_bank_slots(6).unwrap();
    doc.set_gun_slots(2).unwrap();
    doc.set_currency(savekit::Currency::Money, 80).unwrap();
    let mut key = 1000;
    let mut keys = move || {
        key += 1;
        key
    };
    doc.import_codes_with(&[GEAR_WEAPON_CODE, ITEM_CODE, ECHO_CODE], false, &mut keys).unwrap();
    doc
}

pub(crate) fn sample_bytes(save_game_id: i64) -> Vec<u8> {
    sample_doc(save_game_id).to_file_bytes().unwrap()
}

/// The gear weapon of [`sample_doc`].
pub(crate) fn weapon_ref() -> EntryRef {
    EntryRef::new(field::WEAPONS, 0)
}

/// The story item of [`sample_doc`].
pub(crate) fn echo_ref() -> EntryRef {
    EntryRef::new(field::ITEMS, 1)
}

/// A process list that says whether the game runs according to a flag the test holds.
pub(crate) struct FakeProcesses {
    pub running: Arc<AtomicBool>,
    pub unreadable: Arc<AtomicBool>,
    pub calls: Arc<AtomicU32>,
}

impl FakeProcesses {
    pub(crate) fn new() -> FakeProcesses {
        FakeProcesses {
            running: Arc::new(AtomicBool::new(false)),
            unreadable: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(AtomicU32::new(0)),
        }
    }

    pub(crate) fn lister(&self) -> crate::ProcessLister {
        let (running, unreadable, calls) = (self.running.clone(), self.unreadable.clone(), self.calls.clone());
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            if unreadable.load(Ordering::SeqCst) {
                return Err(GkError("refused: cannot list running processes".to_string()));
            }
            let mut names = vec!["explorer.exe".to_string(), "steam.exe".to_string()];
            if running.load(Ordering::SeqCst) {
                names.push("Borderlands2.exe".to_string());
            }
            Ok(names)
        })
    }
}

/// File names in `dir`, sorted.
pub(crate) fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}
