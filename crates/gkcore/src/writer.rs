//! The safe write path: the one function through which every write to SaveData goes.
//!
//! [`safe_write`] runs these steps in order and stops at the first failure with nothing
//! changed:
//!
//! 1. policy: may this file be written at all?
//! 2. the game must not be running (and "cannot tell" counts as running);
//! 3. the file on disk must still be the bytes the caller started from;
//! 4. the original is copied to a new backup that never overwrites an earlier one;
//! 5. the new save is written to a temp file in the target's folder;
//! 6. the temp file is read back from disk and verified;
//! 7. the temp file replaces the target in one step;
//! 8. the target is read back and must equal what was verified.
//!
//! A leftover temp file is removed on every failure. A backup, once made, stays.
//!
//! The same function creates new character files ([`WriteMode::CreateNew`]). There the
//! policy is "a free slot, 2 to 9, with a save-game ID equal to the slot and used by no
//! other save", there is nothing to back up, and step 7 places the file so that it cannot
//! replace anything.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use savekit::{EntryRef, Expectation, SaveDocument};

use crate::changes::LevelFree;
use crate::backup::{BackupInfo, BackupStore};
use crate::error::{fail, from_guard, CoreError, ErrorKind, Result};
use crate::guards::{self, GkError};
use crate::library;
use crate::policy::{self, WriteAccess};
use crate::settings::Settings;
use crate::slot::SlotId;

/// Gives the image names of the running processes (such as `Borderlands2.exe`). An error
/// means the list could not be read, which is treated as "the game may be running".
pub type ProcessLister = Arc<dyn Fn() -> std::result::Result<Vec<String>, GkError> + Send + Sync>;

/// The real process list, from `tasklist`.
pub fn live_process_lister() -> ProcessLister {
    Arc::new(guards::live_process_names)
}

/// Whether the game is running, as far as can be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameStatus {
    NotRunning,
    Running,
    /// The process list could not be read; the reason is for the user. Writes are refused.
    Unknown(String),
}

pub(crate) const GAME_RUNNING_MESSAGE: &str = "Borderlands 2 is running. Quit the game, then save.";

pub(crate) fn game_status(processes: &dyn Fn() -> std::result::Result<Vec<String>, GkError>) -> GameStatus {
    match processes() {
        Err(e) => GameStatus::Unknown(from_guard(ErrorKind::GameStatusUnknown, e).message),
        // A real process list is never empty, so an empty one proves nothing.
        Ok(names) if names.is_empty() => GameStatus::Unknown("The list of running programs came back empty".to_string()),
        Ok(names) if guards::game_is_running(&names) => GameStatus::Running,
        Ok(_) => GameStatus::NotRunning,
    }
}

fn check_game(env: &WriteEnv) -> Result<()> {
    match game_status(env.processes) {
        GameStatus::NotRunning => Ok(()),
        GameStatus::Running => fail(ErrorKind::GameRunning, GAME_RUNNING_MESSAGE),
        GameStatus::Unknown(why) => fail(
            ErrorKind::GameStatusUnknown,
            format!("Vault Key can't tell whether Borderlands 2 is running ({why}), so nothing was written. Try again."),
        ),
    }
}

/// The steps of a write, in order. Reported as each one starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteStep {
    Policy,
    GameCheck,
    UnchangedCheck,
    Backup,
    WriteTemp,
    Verify,
    Replace,
    Confirm,
}

impl WriteStep {
    pub const ALL: [WriteStep; 8] = [
        WriteStep::Policy,
        WriteStep::GameCheck,
        WriteStep::UnchangedCheck,
        WriteStep::Backup,
        WriteStep::WriteTemp,
        WriteStep::Verify,
        WriteStep::Replace,
        WriteStep::Confirm,
    ];

    /// 1 to 8.
    pub fn number(self) -> u8 {
        match self {
            WriteStep::Policy => 1,
            WriteStep::GameCheck => 2,
            WriteStep::UnchangedCheck => 3,
            WriteStep::Backup => 4,
            WriteStep::WriteTemp => 5,
            WriteStep::Verify => 6,
            WriteStep::Replace => 7,
            WriteStep::Confirm => 8,
        }
    }

    /// What the step does, for a progress display.
    pub fn label(self) -> &'static str {
        match self {
            WriteStep::Policy => "Checking this file may be written",
            WriteStep::GameCheck => "Checking the game is closed",
            WriteStep::UnchangedCheck => "Checking the file has not changed",
            WriteStep::Backup => "Backing up the original",
            WriteStep::WriteTemp => "Writing the new save",
            WriteStep::Verify => "Verifying the new save",
            WriteStep::Replace => "Putting the new save in place",
            WriteStep::Confirm => "Confirming the result",
        }
    }
}

/// Something the written file must satisfy that the engine's verifier cannot express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    /// The written document equals this one exactly.
    Equals(SaveDocument),
    /// For each item (compared without its level), exactly this many more entries hold it
    /// than in the baseline; and the number of real (non-placeholder) entries grew by exactly
    /// the total.
    InventoryGrowth { added: Vec<(LevelFree, usize)> },
    PlaythroughsAtLeast(i64),
    SlaughterdomeUnlocked,
    /// The entry decodes and has this level.
    ItemLevel(EntryRef, u32),
    BackpackAtLeast(i64),
    BankAtLeast(i64),
}

impl Check {
    pub(crate) fn run(&self, baseline: &SaveDocument, written: &SaveDocument) -> std::result::Result<(), String> {
        match self {
            Check::Equals(expected) => {
                if written.message != expected.message {
                    return Err("the written save is not the edited character".to_string());
                }
            }
            Check::InventoryGrowth { added } => {
                let before = baseline.inventory();
                let after = written.inventory();
                let holding = |inventory: &[savekit::InventoryEntry], item: &LevelFree| {
                    inventory.iter().filter(|e| LevelFree::of(e).as_ref() == Some(item)).count()
                };
                for (item, count) in added {
                    let grew = holding(&after, item).checked_sub(holding(&before, item));
                    if grew != Some(*count) {
                        return Err(format!("expected {count} new copies of an item, found {}", grew.map_or("fewer".to_string(), |n| n.to_string())));
                    }
                }
                let real = |inventory: &[savekit::InventoryEntry]| inventory.iter().filter(|e| !e.is_virtual).count();
                let total: usize = added.iter().map(|(_, count)| count).sum();
                if real(&after) != real(&before) + total {
                    return Err(format!(
                        "expected {} items in all, found {}",
                        real(&before) + total,
                        real(&after)
                    ));
                }
            }
            Check::PlaythroughsAtLeast(least) => {
                if written.playthroughs_unlocked() < *least {
                    return Err("the game mode unlock is missing".to_string());
                }
            }
            Check::SlaughterdomeUnlocked => {
                if !written.slaughterdome_unlocked() {
                    return Err("the Creature Slaughter Dome unlock is missing".to_string());
                }
            }
            Check::ItemLevel(entry, level) => {
                let fields = written.inventory().into_iter().find(|e| e.entry_ref == *entry).and_then(|e| e.fields);
                let Some(fields) = fields else {
                    return Err(format!("item {entry} is missing or unreadable"));
                };
                if fields.grade_index != Some(*level) || fields.game_stage.is_some_and(|stage| stage != *level) {
                    return Err(format!("item {entry} does not have level {level}"));
                }
            }
            Check::BackpackAtLeast(least) => {
                if written.backpack_slots() < *least {
                    return Err(format!("the backpack has fewer than {least} slots"));
                }
            }
            Check::BankAtLeast(least) => {
                if written.bank_slots() < *least {
                    return Err(format!("the bank has fewer than {least} slots"));
                }
            }
        }
        Ok(())
    }
}

/// What kind of write this is.
pub(crate) enum WriteMode<'a> {
    /// Replace an existing file, which must still hold exactly these bytes.
    Replace { expected_on_disk: &'a [u8] },
    /// Create the file for this slot, which must be free.
    CreateNew { slot: u32 },
}

pub(crate) struct WriteRequest<'a> {
    pub slot_id: &'a SlotId,
    pub mode: WriteMode<'a>,
    /// The bytes of the new save file.
    pub payload: &'a [u8],
    /// What the engine's verifier compares the written file with.
    pub baseline: &'a SaveDocument,
    pub expectations: &'a [Expectation],
    pub checks: &'a [Check],
    /// Why the backup is taken, such as "Before saving 3 changes".
    pub reason: &'a str,
}

/// Everything the write path needs from the outside.
pub(crate) struct WriteEnv<'a> {
    /// The SaveData folder being worked in.
    pub save_root: &'a Path,
    /// Other SaveData folders (the real ones, when `save_root` is an override).
    pub other_save_roots: &'a [PathBuf],
    pub settings: &'a Settings,
    pub backups: &'a BackupStore,
    pub processes: &'a dyn Fn() -> std::result::Result<Vec<String>, GkError>,
    /// Makes the temp file's name unique.
    pub unique: &'a str,
}

#[derive(Debug)]
pub(crate) struct WriteOutcome {
    pub path: PathBuf,
    /// The backup of the replaced file. `None` when a new file was created.
    pub backup: Option<BackupInfo>,
}

/// Removes the temp file when dropped, whatever happened.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn same_place(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// The folder and path of the file `slot_id` names, after checking that it really is
/// `<save_root>/<steam id>/<file name>` and not a link to somewhere else.
pub(crate) fn resolve_target(save_root: &Path, slot_id: &SlotId) -> Result<(PathBuf, PathBuf)> {
    // Checked when the id was made; checked again here because this is the gate.
    let slot_id = SlotId::new(slot_id.steam_id(), slot_id.file_name())?;
    let dir = save_root.join(slot_id.steam_id());
    if !dir.is_dir() {
        return fail(
            ErrorKind::InvalidTarget,
            format!("There is no folder {} in the game's save folder.", slot_id.steam_id()),
        );
    }
    let target = dir.join(slot_id.file_name());
    let expected = guards::physical_path(save_root).join(slot_id.steam_id()).join(slot_id.file_name());
    if !same_place(&guards::physical_path(&target), &expected) {
        return fail(
            ErrorKind::InvalidTarget,
            format!("{} is not directly inside the game's save folder (it is a link to somewhere else).", target.display()),
        );
    }
    Ok((dir, target))
}

/// Name of another save in `dir` that uses save-game ID `id`. Fails closed: an error if the
/// folder cannot be listed or any other save cannot be decoded, since it might hold the ID.
fn neighbour_using_id(dir: &Path, skip_file: &str, id: i64) -> Result<Option<String>> {
    let Some(names) = library::save_file_names(dir) else {
        return fail(ErrorKind::IdCollision, format!("Cannot list {} to check save-game IDs.", dir.display()));
    };
    for name in names {
        if name.eq_ignore_ascii_case(skip_file) {
            continue;
        }
        let decoded = fs::read(dir.join(&name)).ok().and_then(|bytes| SaveDocument::from_file_bytes(&bytes).ok());
        match decoded {
            None => {
                return fail(
                    ErrorKind::IdCollision,
                    format!("Cannot read {name} in the save folder, so its save-game ID is unknown. Fix or move that file first."),
                )
            }
            Some(doc) if doc.save_game_id() == id => return Ok(Some(name)),
            Some(_) => {}
        }
    }
    Ok(None)
}

fn collision(other: &str, id: i64) -> CoreError {
    CoreError::new(
        ErrorKind::IdCollision,
        format!("{other} already uses save-game ID {id}. Two saves with the same ID could overwrite each other, so nothing was written."),
    )
}

/// Step 1 for a new file: the slot rules that stand in for the write policy.
fn check_new_slot(env: &WriteEnv, req: &WriteRequest, slot: u32, dir: &Path, target: &Path, new: &SaveDocument) -> Result<()> {
    if !policy::is_creatable_slot(slot) {
        return fail(
            ErrorKind::InvalidTarget,
            format!(
                "Vault Key can only save to slots {} to {} (higher slots use file names Vault Key does not handle yet).",
                guards::MIN_SLOT,
                guards::MAX_SLOT
            ),
        );
    }
    let file_name = guards::slot_file_name(slot);
    if !req.slot_id.file_name().eq_ignore_ascii_case(&file_name) {
        return fail(ErrorKind::InvalidTarget, format!("Slot {slot}'s file must be named {file_name}."));
    }
    if fs::symlink_metadata(target).is_ok() {
        return fail(ErrorKind::SlotTaken, format!("Slot {slot} already has a character ({file_name}). Pick a free slot."));
    }
    let id = new.save_game_id();
    if id != i64::from(slot) {
        return fail(
            ErrorKind::InvalidTarget,
            format!("The save-game ID ({id}) must equal the slot number ({slot})."),
        );
    }
    let mut roots = vec![env.save_root.to_path_buf()];
    roots.extend(env.other_save_roots.iter().cloned());
    if guards::is_real_save_0001(req.payload, &roots) {
        return fail(
            ErrorKind::InvalidTarget,
            "This is an exact copy of a Save0001.sav in the save folder. Vault Key never installs a copy of the slot 1 character.",
        );
    }
    let id_of = |bytes: &[u8]| SaveDocument::from_file_bytes(bytes).ok().and_then(|d| u64::try_from(d.save_game_id()).ok());
    match guards::slot_using_save_game_id(u64::from(slot), dir, &id_of) {
        Ok(None) => Ok(()),
        Ok(Some(other)) => Err(collision(&other, id)),
        Err(e) => Err(from_guard(ErrorKind::IdCollision, e)),
    }
}

fn unverified(why: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        ErrorKind::VerificationFailed,
        format!("The new save did not pass verification, so nothing was changed ({why})."),
    )
}

/// The single write path. See the module documentation for the steps.
///
/// `on_step` is told each step as it starts.
pub(crate) fn safe_write(env: &WriteEnv, req: &WriteRequest, on_step: &mut dyn FnMut(WriteStep)) -> Result<WriteOutcome> {
    // 1. Policy.
    on_step(WriteStep::Policy);
    let (dir, target) = resolve_target(env.save_root, req.slot_id)?;
    if let WriteMode::Replace { expected_on_disk } = req.mode {
        // A test-slot mark covers one character. Which character is in the file is read from
        // the bytes the caller expects on disk; step 3 refuses if the file holds anything else.
        // If those bytes are not a readable save (a damaged file), the mark still covers
        // putting the marked character itself back, which is what restoring a backup does.
        let on_disk = SaveDocument::from_file_bytes(expected_on_disk).ok();
        let incoming = SaveDocument::from_file_bytes(req.payload).ok();
        // A protected character is never replaced, and never put back over anything either.
        if [&on_disk, &incoming].into_iter().flatten().any(|d| policy::is_protected(env.settings, d)) {
            return fail(ErrorKind::ReadOnly, policy::PROTECTED_MESSAGE);
        }
        let character = on_disk.as_ref().or(incoming.as_ref()).map(policy::character_mark);
        if let WriteAccess::ReadOnly(why) = policy::access(env.settings, req.slot_id, character.as_deref()) {
            return fail(ErrorKind::ReadOnly, why);
        }
    }
    let new = SaveDocument::from_file_bytes(req.payload).map_err(unverified)?;
    if let WriteMode::CreateNew { slot } = req.mode {
        check_new_slot(env, req, slot, &dir, &target, &new)?;
    }

    // 2. The game must be closed.
    on_step(WriteStep::GameCheck);
    check_game(env)?;

    // 3. The file must be what the caller started from.
    on_step(WriteStep::UnchangedCheck);
    let current: Option<Vec<u8>> = match req.mode {
        WriteMode::Replace { expected_on_disk } => {
            let on_disk = fs::read(&target).map_err(|e| {
                let what = if e.kind() == std::io::ErrorKind::NotFound {
                    "This save file is no longer there".to_string()
                } else {
                    format!("This save file can no longer be read ({e})")
                };
                CoreError::new(ErrorKind::ChangedOnDisk, format!("{what}. Reload the character list. Nothing was changed."))
            })?;
            if on_disk != expected_on_disk {
                return fail(
                    ErrorKind::ChangedOnDisk,
                    "This save changed on disk since Vault Key opened it (did the game save?). Reload it and redo your changes.",
                );
            }
            // Replacing must never bring in a save-game ID that another file uses.
            let id = new.save_game_id();
            let same_id = SaveDocument::from_file_bytes(&on_disk).is_ok_and(|old| old.save_game_id() == id);
            if !same_id {
                if let Some(other) = neighbour_using_id(&dir, req.slot_id.file_name(), id)? {
                    return Err(collision(&other, id));
                }
            }
            Some(on_disk)
        }
        WriteMode::CreateNew { slot } => {
            if fs::symlink_metadata(&target).is_ok() {
                return fail(
                    ErrorKind::SlotTaken,
                    format!("Slot {slot} already has a character ({}). Pick a free slot.", req.slot_id.file_name()),
                );
            }
            None
        }
    };

    // 4. Back up the original. From here on the backup stays, whatever happens.
    on_step(WriteStep::Backup);
    let backup = match &current {
        Some(original) => Some(env.backups.create(req.slot_id, &target, original, req.reason)?),
        None => None,
    };

    // 5. Write the new save to a temp file next to the target.
    on_step(WriteStep::WriteTemp);
    let temp_path = dir.join(format!(".gk-{}.tmp", env.unique));
    let not_written = |e: std::io::Error| {
        CoreError::new(
            ErrorKind::WriteFailed,
            format!("Could not write the new save next to the old one, so nothing was changed ({e})."),
        )
    };
    // `create_new` fails if the name is taken, so this never writes over anything.
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).map_err(not_written)?;
    let temp = TempFile(temp_path);
    file.write_all(req.payload).and_then(|()| file.sync_all()).map_err(not_written)?;
    drop(file);

    // 6. Read the temp file back from disk and verify it.
    on_step(WriteStep::Verify);
    let verified = fs::read(&temp.0).map_err(|e| unverified(format!("cannot read the temporary file back: {e}")))?;
    if verified != req.payload {
        return Err(unverified("the temporary file read back differently"));
    }
    savekit::verify(req.baseline, &verified, req.expectations).map_err(unverified)?;
    let written = SaveDocument::from_file_bytes(&verified).map_err(unverified)?;
    for check in req.checks {
        check.run(req.baseline, &written).map_err(unverified)?;
    }

    // 7. Put it in place, in one step. The game is checked once more first.
    on_step(WriteStep::Replace);
    check_game(env)?;
    match req.mode {
        WriteMode::Replace { .. } => fs::rename(&temp.0, &target).map_err(|e| {
            CoreError::new(
                ErrorKind::ReplaceFailed,
                format!("Could not put the new save in place, so nothing was changed ({e})."),
            )
        })?,
        // A hard link fails if the name is taken, so a new character can never replace a file.
        WriteMode::CreateNew { slot } => fs::hard_link(&temp.0, &target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                CoreError::new(ErrorKind::SlotTaken, format!("Slot {slot} was taken while saving. Nothing was changed."))
            } else {
                CoreError::new(
                    ErrorKind::ReplaceFailed,
                    format!("Could not create the new save file, so nothing was changed ({e})."),
                )
            }
        })?,
    }

    // 8. Read the target back: it must be exactly what was verified.
    on_step(WriteStep::Confirm);
    let confirmed = fs::read(&target).is_ok_and(|on_disk| on_disk == verified);
    if !confirmed {
        return Err(undo(env, req, &target, current.as_deref(), backup.as_ref()));
    }
    Ok(WriteOutcome { path: target, backup })
}

/// Step 8 failed: the file in place is not the verified one. Puts things back as they were
/// and says what happened.
fn undo(env: &WriteEnv, req: &WriteRequest, target: &Path, original: Option<&[u8]>, backup: Option<&BackupInfo>) -> CoreError {
    let name = req.slot_id.file_name();
    let message = match original {
        None => match fs::remove_file(target) {
            Ok(()) => format!("The new save file {name} did not read back as written, so it was removed. Nothing else was changed."),
            Err(e) => format!("The new save file {name} did not read back as written and could not be removed ({e}). Delete it by hand before starting the game."),
        },
        Some(original) => {
            let undo_path = target.with_file_name(format!(".gk-{}-undo.tmp", env.unique));
            let put_back = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&undo_path)
                .and_then(|mut file| {
                    file.write_all(original)?;
                    file.sync_all()
                })
                .and_then(|()| fs::rename(&undo_path, target));
            let restored = put_back.is_ok() && fs::read(target).is_ok_and(|on_disk| on_disk == original);
            let _ = fs::remove_file(&undo_path);
            if restored {
                format!("{name} did not read back as written, so the original was put back. Nothing was changed.")
            } else {
                let place = backup.map_or_else(|| "the backups folder".to_string(), |b| b.path.clone());
                format!("{name} did not read back as written, and the original could not be put back. Your original is safe at {place}: restore it from the backups list before starting the game.")
            }
        }
    };
    CoreError::new(ErrorKind::ConfirmFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{file_names, sample_bytes, sample_doc, FakeProcesses, TempDir, STEAM_ID};
    use crate::time;
    use savekit::{CharacterKey, Currency};
    use std::sync::atomic::Ordering;

    const NOW: u64 = 1_794_194_100;

    /// A temp SaveData folder, a temp backups folder, settings and a fake process list.
    struct World {
        temp: TempDir,
        root: PathBuf,
        dir: PathBuf,
        settings: Settings,
        backups: BackupStore,
        processes: FakeProcesses,
        lister: ProcessLister,
    }

    impl World {
        fn new() -> World {
            let temp = TempDir::new();
            let root = temp.path().join("SaveData");
            let dir = root.join(STEAM_ID);
            fs::create_dir_all(&dir).unwrap();
            let backups = BackupStore::new(temp.path().join("data").join("backups"), time::fixed_clock(NOW));
            let processes = FakeProcesses::new();
            let lister = processes.lister();
            World { temp, root, dir, settings: Settings::default(), backups, processes, lister }
        }

        fn env(&self) -> WriteEnv<'_> {
            WriteEnv {
                save_root: &self.root,
                other_save_roots: &[],
                settings: &self.settings,
                backups: &self.backups,
                processes: &*self.lister,
                unique: "test",
            }
        }

        /// Puts a sample character in a slot file and returns its bytes.
        fn put(&self, file: &str, save_game_id: i64) -> Vec<u8> {
            let bytes = sample_bytes(save_game_id);
            fs::write(self.dir.join(file), &bytes).unwrap();
            bytes
        }

        /// The slot file's id, recorded as created by the app.
        fn writable(&mut self, file: &str) -> SlotId {
            let slot = SlotId::new(STEAM_ID, file).unwrap();
            self.settings.record_created(&slot);
            slot
        }
    }

    /// An in-place edit of a sample character: money set to 999.
    struct MoneyEdit {
        baseline: SaveDocument,
        payload: Vec<u8>,
        expectations: Vec<Expectation>,
        checks: Vec<Check>,
    }

    fn money_edit(save_game_id: i64) -> MoneyEdit {
        let baseline = sample_doc(save_game_id);
        let mut edited = baseline.clone();
        edited.set_currency(Currency::Money, 999).unwrap();
        MoneyEdit {
            payload: edited.to_file_bytes().unwrap(),
            expectations: vec![Expectation::CharacterValue(CharacterKey::Money, 999)],
            checks: vec![Check::Equals(edited)],
            baseline,
        }
    }

    impl MoneyEdit {
        fn replace<'a>(&'a self, slot_id: &'a SlotId, expected_on_disk: &'a [u8]) -> WriteRequest<'a> {
            WriteRequest {
                slot_id,
                mode: WriteMode::Replace { expected_on_disk },
                payload: &self.payload,
                baseline: &self.baseline,
                expectations: &self.expectations,
                checks: &self.checks,
                reason: "Before saving 1 change",
            }
        }
    }

    fn run(world: &World, req: &WriteRequest) -> (Result<WriteOutcome>, Vec<WriteStep>) {
        let mut steps = Vec::new();
        let result = safe_write(&world.env(), req, &mut |step| steps.push(step));
        (result, steps)
    }

    /// The state every refused write must leave behind: the target as it was, and nothing
    /// else in its folder that was not there before.
    fn assert_untouched(world: &World, file: &str, before: &[u8], names_before: &[String]) {
        assert_eq!(fs::read(world.dir.join(file)).unwrap(), before, "the target must be byte-identical");
        assert_eq!(file_names(&world.dir), names_before, "no temp file may remain");
    }

    #[test]
    fn replace_runs_all_eight_steps_and_keeps_a_backup_of_the_original() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let other = world.put("Save0004.sav", 4);
        let slot = world.writable("Save0003.sav");
        let edit = money_edit(3);

        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let outcome = result.unwrap();
        assert_eq!(steps, WriteStep::ALL);
        assert_eq!(outcome.path, world.dir.join("Save0003.sav"));
        assert_eq!(fs::read(&outcome.path).unwrap(), edit.payload);
        assert_eq!(SaveDocument::from_file_bytes(&fs::read(&outcome.path).unwrap()).unwrap().money(), 999);
        let backup = outcome.backup.unwrap();
        assert_eq!(fs::read(&backup.path).unwrap(), original);
        assert_eq!(backup.reason, "Before saving 1 change");
        assert_eq!(world.backups.list().len(), 1);
        assert_eq!(file_names(&world.dir), ["Save0003.sav", "Save0004.sav"]);
        assert_eq!(fs::read(world.dir.join("Save0004.sav")).unwrap(), other);
        // The game was checked at step 2 and again just before the replace.
        assert_eq!(world.processes.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn step_1_policy_refuses_any_file_not_created_by_the_app_or_marked() {
        let mut world = World::new();
        let original = world.put("Save0005.sav", 5);
        let first = world.put("Save0001.sav", 1);
        let copy = world.put("Save0005 - Copy.sav", 5);
        // Another slot being writable does not help.
        world.writable("Save0003.sav");
        let names = file_names(&world.dir);

        let edit = money_edit(5);
        let slot = SlotId::new(STEAM_ID, "Save0005.sav").unwrap();
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::ReadOnly);
        assert_eq!(error.message, policy::READ_ONLY_MESSAGE);
        assert_eq!(steps, [WriteStep::Policy], "a refusal stops at the first step");
        assert_untouched(&world, "Save0005.sav", &original, &names);

        // Slot 1 is refused when a hand-edited settings file claims it: Vault Key never
        // creates slot 1, and a test-slot line must say which character it is for.
        world.settings = Settings::parse(&format!("created {STEAM_ID}/Save0001.sav\ntest-slot {STEAM_ID}/Save0001.sav\n"));
        let edit = money_edit(1);
        let slot = SlotId::new(STEAM_ID, "Save0001.sav").unwrap();
        let (result, _) = run(&world, &edit.replace(&slot, &first));
        assert_eq!(result.unwrap_err().kind, ErrorKind::ReadOnly);
        assert_untouched(&world, "Save0001.sav", &first, &names);

        // A test-slot mark that was made for another character does not cover this one.
        world.settings = Settings::default();
        world.settings.mark_test_slot(&slot, "00000000000000aa");
        let (result, steps) = run(&world, &edit.replace(&slot, &first));
        let error = result.unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::ReadOnly, policy::OTHER_CHARACTER_MESSAGE));
        assert_eq!(steps, [WriteStep::Policy]);
        assert_untouched(&world, "Save0001.sav", &first, &names);

        // A file without a slot name is refused whatever the settings say.
        world.settings = Settings::parse(&format!("test-slot {STEAM_ID}/Save0005 - Copy.sav\n"));
        let slot = SlotId::new(STEAM_ID, "Save0005 - Copy.sav").unwrap();
        let (result, _) = run(&world, &money_edit(5).replace(&slot, &copy));
        assert_eq!(result.unwrap_err().kind, ErrorKind::ReadOnly);
        assert_untouched(&world, "Save0005 - Copy.sav", &copy, &names);

        assert!(world.backups.list().is_empty(), "a refused write makes no backup");
        assert_eq!(world.processes.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn step_1_marking_a_test_slot_is_what_makes_an_existing_file_writable() {
        let mut world = World::new();
        let original = world.put("Save0005.sav", 5);
        let slot = SlotId::new(STEAM_ID, "Save0005.sav").unwrap();
        let edit = money_edit(5);
        assert_eq!(run(&world, &edit.replace(&slot, &original)).0.unwrap_err().kind, ErrorKind::ReadOnly);
        world.settings.mark_test_slot(&SlotId::new(STEAM_ID, "SAVE0005.SAV").unwrap(), &policy::character_mark(&sample_doc(5)));
        run(&world, &edit.replace(&slot, &original)).0.unwrap();
        assert_eq!(fs::read(world.dir.join("Save0005.sav")).unwrap(), edit.payload);
    }

    #[test]
    fn step_1_slot_1_is_writable_only_as_a_marked_test_slot() {
        let mut world = World::new();
        let original = world.put("Save0001.sav", 1);
        let slot = SlotId::new(STEAM_ID, "Save0001.sav").unwrap();
        let edit = money_edit(1);
        assert_eq!(run(&world, &edit.replace(&slot, &original)).0.unwrap_err().kind, ErrorKind::ReadOnly);
        world.settings.mark_test_slot(&slot, &policy::character_mark(&sample_doc(1)));
        let outcome = run(&world, &edit.replace(&slot, &original)).0.unwrap();
        assert!(outcome.backup.is_some(), "the old slot 1 file is backed up first");
        assert_eq!(fs::read(world.dir.join("Save0001.sav")).unwrap(), edit.payload);
    }

    #[test]
    fn step_1_refuses_targets_outside_the_save_folder() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let edit = money_edit(3);
        // A Steam-ID folder that is not there.
        let elsewhere = SlotId::new("111", "Save0003.sav").unwrap();
        world.settings.record_created(&elsewhere);
        let (result, steps) = run(&world, &edit.replace(&elsewhere, &original));
        assert_eq!(result.unwrap_err().kind, ErrorKind::InvalidTarget);
        assert_eq!(steps, [WriteStep::Policy]);

        // A Steam-ID folder that is really a junction to a folder outside SaveData.
        let outside = world.temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("Save0003.sav"), &original).unwrap();
        let link = world.root.join("222");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !made {
            eprintln!("skipped: could not create a junction here");
            return;
        }
        let linked = SlotId::new("222", "Save0003.sav").unwrap();
        world.settings.record_created(&linked);
        let (result, _) = run(&world, &edit.replace(&linked, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidTarget, "{}", error.message);
        assert_eq!(fs::read(outside.join("Save0003.sav")).unwrap(), original);
        assert_eq!(file_names(&outside), ["Save0003.sav"]);
        let _ = fs::remove_dir(&link);
    }

    #[test]
    fn step_2_refuses_while_the_game_runs_or_when_it_cannot_tell() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        let edit = money_edit(3);

        world.processes.running.store(true, Ordering::SeqCst);
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::GameRunning, GAME_RUNNING_MESSAGE));
        assert_eq!(steps, [WriteStep::Policy, WriteStep::GameCheck]);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // The process list cannot be read: that is not proof the game is closed.
        world.processes.running.store(false, Ordering::SeqCst);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::GameStatusUnknown);
        assert!(error.message.contains("can't tell whether Borderlands 2 is running"), "{}", error.message);
        assert_eq!(steps, [WriteStep::Policy, WriteStep::GameCheck]);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // Nor is an empty list.
        world.lister = Arc::new(|| Ok(Vec::new()));
        let (result, _) = run(&world, &edit.replace(&slot, &original));
        assert_eq!(result.unwrap_err().kind, ErrorKind::GameStatusUnknown);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        assert!(world.backups.list().is_empty());
    }

    #[test]
    fn step_3_refuses_when_the_file_changed_on_disk_since_it_was_opened() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let edit = money_edit(3);

        // The game saved in the meantime: same character, different bytes.
        let mut played = sample_doc(3);
        played.set_level(6).unwrap();
        let played = played.to_file_bytes().unwrap();
        fs::write(world.dir.join("Save0003.sav"), &played).unwrap();
        let names = file_names(&world.dir);
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::ChangedOnDisk);
        assert!(error.message.contains("changed on disk since Vault Key opened it"));
        assert_eq!(steps, [WriteStep::Policy, WriteStep::GameCheck, WriteStep::UnchangedCheck]);
        assert_untouched(&world, "Save0003.sav", &played, &names);

        // The file was deleted in the meantime: nothing is created in its place.
        fs::remove_file(world.dir.join("Save0003.sav")).unwrap();
        let (result, _) = run(&world, &edit.replace(&slot, &original));
        assert_eq!(result.unwrap_err().kind, ErrorKind::ChangedOnDisk);
        assert!(file_names(&world.dir).is_empty());

        assert!(world.backups.list().is_empty(), "no backup is made of a file that will not be written");
    }

    #[test]
    fn step_3_refuses_a_replacement_that_brings_in_another_saves_id() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        world.put("Save0004.sav", 4);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        // What a restore of a strange backup would look like: the new bytes carry ID 4.
        let baseline = sample_doc(4);
        let payload = baseline.to_file_bytes().unwrap();
        let req = WriteRequest {
            slot_id: &slot,
            mode: WriteMode::Replace { expected_on_disk: &original },
            payload: &payload,
            baseline: &baseline,
            expectations: &[Expectation::Unchanged],
            checks: &[],
            reason: "Before restoring",
        };
        let error = run(&world, &req).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::IdCollision);
        assert!(error.message.contains("Save0004.sav already uses save-game ID 4"), "{}", error.message);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // And with a neighbour that cannot be read, it cannot be ruled out, so: refused.
        fs::write(world.dir.join("Save0004.sav"), b"not a save").unwrap();
        let error = run(&world, &req).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::IdCollision);
        assert!(error.message.contains("Cannot read Save0004.sav"), "{}", error.message);
        assert_eq!(fs::read(world.dir.join("Save0003.sav")).unwrap(), original);
        assert!(world.backups.list().is_empty());
    }

    #[test]
    fn step_4_a_backup_that_cannot_be_made_stops_the_write() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        // A plain file where the backups folder should be.
        fs::create_dir_all(world.temp.path().join("data")).unwrap();
        fs::write(world.temp.path().join("data").join("backups"), b"in the way").unwrap();

        let edit = money_edit(3);
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::BackupFailed);
        assert_eq!(steps.last(), Some(&WriteStep::Backup));
        assert_untouched(&world, "Save0003.sav", &original, &names);
        assert_eq!(fs::read(world.temp.path().join("data").join("backups")).unwrap(), b"in the way");
    }

    #[test]
    fn step_5_a_temp_file_that_cannot_be_written_stops_the_write() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        // Something already has the temp file's name. It is not ours, so it must survive.
        fs::create_dir(world.dir.join(".gk-test.tmp")).unwrap();
        let names = file_names(&world.dir);

        let edit = money_edit(3);
        let (result, steps) = run(&world, &edit.replace(&slot, &original));
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::WriteFailed);
        assert_eq!(steps.last(), Some(&WriteStep::WriteTemp));
        assert_untouched(&world, "Save0003.sav", &original, &names);
        assert!(world.dir.join(".gk-test.tmp").is_dir(), "what was in the way is left alone");
        // The backup was made before the failure, and it stays.
        let backups = world.backups.list();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0].path).unwrap(), original);
    }

    #[test]
    fn step_6_a_save_that_fails_verification_is_never_put_in_place() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        let edit = money_edit(3);

        // The intended change is not in the file: an expectation fails.
        let wrong = [Expectation::CharacterValue(CharacterKey::Money, 1)];
        let mut req = edit.replace(&slot, &original);
        req.expectations = &wrong;
        let (result, steps) = run(&world, &req);
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::VerificationFailed);
        assert!(error.message.contains("money: expected 1, found 999"), "{}", error.message);
        assert_eq!(steps.last(), Some(&WriteStep::Verify));
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // The file is not the edited character: Vault Key's own check fails.
        let other = [Check::Equals(sample_doc(3))];
        let mut req = edit.replace(&slot, &original);
        req.checks = &other;
        let (result, steps) = run(&world, &req);
        assert_eq!(result.unwrap_err().kind, ErrorKind::VerificationFailed);
        assert_eq!(steps.last(), Some(&WriteStep::Verify));
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // An item the original had is damaged in the new file: the engine's standing check fails.
        let mut damaged = sample_doc(3);
        let entry = damaged.message.fields.iter_mut().find(|f| f.number == savekit::field::WEAPONS).unwrap();
        let mut sub = savekit::ProtoMessage::decode(entry.value.as_bytes().unwrap()).unwrap();
        let mut blob = sub.bytes(1).unwrap().to_vec();
        blob[8] ^= 1;
        sub.set_single(1, savekit::ProtoValue::Bytes(blob));
        entry.value = savekit::ProtoValue::Bytes(sub.encode());
        let damaged = damaged.to_file_bytes().unwrap();
        let mut req = edit.replace(&slot, &original);
        req.payload = &damaged;
        req.expectations = &[];
        req.checks = &[];
        let (result, _) = run(&world, &req);
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::VerificationFailed);
        assert!(error.message.contains("failed its checksum"), "{}", error.message);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // Bytes that are not a save at all never get as far as a temp file.
        let mut req = edit.replace(&slot, &original);
        req.payload = b"not a save";
        let (result, steps) = run(&world, &req);
        assert_eq!(result.unwrap_err().kind, ErrorKind::VerificationFailed);
        assert_eq!(steps, [WriteStep::Policy]);
        assert_untouched(&world, "Save0003.sav", &original, &names);

        // Each attempt that reached step 4 left its backup of the original.
        let backups = world.backups.list();
        assert_eq!(backups.len(), 3);
        assert!(backups.iter().all(|b| fs::read(&b.path).unwrap() == original));
    }

    #[test]
    fn step_7_refuses_if_the_game_was_started_during_the_write() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        let edit = money_edit(3);

        let running = world.processes.running.clone();
        let mut steps = Vec::new();
        let result = safe_write(&world.env(), &edit.replace(&slot, &original), &mut |step| {
            steps.push(step);
            if step == WriteStep::Verify {
                // The game is launched while the temp file is being verified.
                running.store(true, Ordering::SeqCst);
            }
        });
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::GameRunning);
        assert_eq!(steps.last(), Some(&WriteStep::Replace));
        assert_untouched(&world, "Save0003.sav", &original, &names);
        assert_eq!(world.backups.list().len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn step_7_a_replace_that_fails_leaves_the_original_in_place() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;

        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        let edit = money_edit(3);

        // Another program opens the save, without allowing it to be replaced, just before
        // the swap.
        let target = world.dir.join("Save0003.sav");
        let mut held = None;
        let mut steps = Vec::new();
        let result = safe_write(&world.env(), &edit.replace(&slot, &original), &mut |step| {
            steps.push(step);
            if step == WriteStep::Replace {
                held = Some(fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&target).unwrap());
            }
        });
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::ReplaceFailed, "{}", error.message);
        assert_eq!(steps.last(), Some(&WriteStep::Replace));
        drop(held);
        assert_untouched(&world, "Save0003.sav", &original, &names);
        assert_eq!(world.backups.list().len(), 1);
    }

    #[test]
    fn step_8_a_file_that_does_not_read_back_is_undone() {
        let mut world = World::new();
        let original = world.put("Save0003.sav", 3);
        let slot = world.writable("Save0003.sav");
        let names = file_names(&world.dir);
        let edit = money_edit(3);

        // Something scribbles on the save between the swap and the read-back.
        let target = world.dir.join("Save0003.sav");
        let mut steps = Vec::new();
        let result = safe_write(&world.env(), &edit.replace(&slot, &original), &mut |step| {
            steps.push(step);
            if step == WriteStep::Confirm {
                fs::write(&target, b"scribbled over").unwrap();
            }
        });
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::ConfirmFailed);
        assert!(error.message.contains("the original was put back"), "{}", error.message);
        assert_eq!(steps, WriteStep::ALL);
        assert_untouched(&world, "Save0003.sav", &original, &names);
        let backups = world.backups.list();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0].path).unwrap(), original);
    }

    // New character files.

    fn new_character<'a>(slot_id: &'a SlotId, slot: u32, baseline: &'a SaveDocument, payload: &'a [u8]) -> WriteRequest<'a> {
        WriteRequest {
            slot_id,
            mode: WriteMode::CreateNew { slot },
            payload,
            baseline,
            expectations: &[],
            checks: &[],
            reason: "",
        }
    }

    #[test]
    fn create_new_places_a_verified_file_in_a_free_slot_without_a_backup() {
        let world = World::new();
        let existing = world.put("Save0001.sav", 1);
        let baseline = sample_doc(1);
        let payload = sample_bytes(3);
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();

        let (result, steps) = run(&world, &new_character(&slot, 3, &baseline, &payload));
        let outcome = result.unwrap();
        assert_eq!(steps, WriteStep::ALL);
        assert!(outcome.backup.is_none() && world.backups.list().is_empty());
        assert_eq!(fs::read(world.dir.join("Save0003.sav")).unwrap(), payload);
        assert_eq!(fs::read(world.dir.join("Save0001.sav")).unwrap(), existing);
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0003.sav"]);

        // The slot is now taken: a second attempt changes nothing.
        let other = {
            let mut doc = sample_doc(3);
            doc.set_level(40).unwrap();
            doc.to_file_bytes().unwrap()
        };
        let (result, steps) = run(&world, &new_character(&slot, 3, &baseline, &other));
        assert_eq!(result.unwrap_err().kind, ErrorKind::SlotTaken);
        assert_eq!(steps, [WriteStep::Policy]);
        assert_eq!(fs::read(world.dir.join("Save0003.sav")).unwrap(), payload);
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0003.sav"]);
    }

    #[test]
    fn create_new_slot_rules() {
        let world = World::new();
        let first = world.put("Save0001.sav", 1);
        world.put("SAVE0009.SAV", 7);
        let names = file_names(&world.dir);
        let baseline = sample_doc(1);
        let attempt = |slot: u32, file: &str, id: i64| {
            let slot_id = SlotId::new(STEAM_ID, file).unwrap();
            let payload = sample_bytes(id);
            let (result, steps) = run(&world, &new_character(&slot_id, slot, &baseline, &payload));
            assert_eq!(steps, [WriteStep::Policy], "slot {slot} {file} id {id}");
            assert_eq!(file_names(&world.dir), names, "slot {slot} {file} id {id}");
            result.unwrap_err()
        };
        // Slot 1, and slots outside 2 to 9.
        assert_eq!(attempt(1, "Save0001.sav", 1).kind, ErrorKind::InvalidTarget);
        assert_eq!(attempt(0, "Save0000.sav", 1).kind, ErrorKind::InvalidTarget);
        assert_eq!(attempt(10, "Save0010.sav", 10).kind, ErrorKind::InvalidTarget);
        assert_eq!(attempt(10, "Save000a.sav", 10).kind, ErrorKind::InvalidTarget);
        // The file name must be the slot's.
        assert_eq!(attempt(3, "Save0004.sav", 3).kind, ErrorKind::InvalidTarget);
        assert_eq!(attempt(3, "Save0003 - Copy.sav", 3).kind, ErrorKind::InvalidTarget);
        // The save-game ID must equal the slot.
        let error = attempt(3, "Save0003.sav", 5);
        assert_eq!(error.kind, ErrorKind::InvalidTarget);
        assert!(error.message.contains("must equal the slot number"), "{}", error.message);
        // Another file, under whatever name, already uses the ID.
        let error = attempt(7, "Save0007.sav", 7);
        assert_eq!(error.kind, ErrorKind::IdCollision);
        assert!(error.message.contains("SAVE0009.SAV already uses save-game ID 7"), "{}", error.message);
        // The slot is taken (Windows file names ignore case).
        if cfg!(windows) {
            assert_eq!(attempt(9, "Save0009.sav", 9).kind, ErrorKind::SlotTaken);
        }
        assert_eq!(fs::read(world.dir.join("Save0001.sav")).unwrap(), first);
    }

    #[test]
    fn create_new_fails_closed_and_never_copies_slot_1() {
        let world = World::new();
        let baseline = sample_doc(1);
        // The slot 1 file happens to carry ID 5, so only the byte comparison catches a copy.
        let first = world.put("Save0001.sav", 5);
        let slot = SlotId::for_slot(STEAM_ID, 5).unwrap();
        let error = run(&world, &new_character(&slot, 5, &baseline, &first)).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidTarget);
        assert!(error.message.contains("exact copy of a Save0001.sav"), "{}", error.message);

        // A neighbour that cannot be decoded might hold the ID.
        fs::write(world.dir.join("Save0007.sav"), b"not a save").unwrap();
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();
        let payload = sample_bytes(3);
        let error = run(&world, &new_character(&slot, 3, &baseline, &payload)).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::IdCollision);
        assert!(error.message.contains("Save0007.sav"), "{}", error.message);
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0007.sav"]);
    }

    #[test]
    fn create_new_is_refused_while_the_game_runs_and_cleans_up_on_failure() {
        let world = World::new();
        let baseline = sample_doc(1);
        let payload = sample_bytes(3);
        let slot = SlotId::for_slot(STEAM_ID, 3).unwrap();

        world.processes.running.store(true, Ordering::SeqCst);
        let error = run(&world, &new_character(&slot, 3, &baseline, &payload)).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::GameRunning);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        let error = run(&world, &new_character(&slot, 3, &baseline, &payload)).0.unwrap_err();
        assert_eq!(error.kind, ErrorKind::GameStatusUnknown);
        world.processes.running.store(false, Ordering::SeqCst);
        world.processes.unreadable.store(false, Ordering::SeqCst);
        assert!(file_names(&world.dir).is_empty());

        // Verification fails: no file, no temp file.
        let wrong = [Expectation::CharacterValue(CharacterKey::Level, 80)];
        let mut req = new_character(&slot, 3, &baseline, &payload);
        req.expectations = &wrong;
        assert_eq!(run(&world, &req).0.unwrap_err().kind, ErrorKind::VerificationFailed);
        assert!(file_names(&world.dir).is_empty());

        // The game creates a character in the slot during the write: it is not replaced.
        let target = world.dir.join("Save0003.sav");
        let result = safe_write(&world.env(), &new_character(&slot, 3, &baseline, &payload), &mut |step| {
            if step == WriteStep::Replace {
                fs::write(&target, b"the game's new character").unwrap();
            }
        });
        assert_eq!(result.unwrap_err().kind, ErrorKind::SlotTaken);
        assert_eq!(fs::read(&target).unwrap(), b"the game's new character");
        assert_eq!(file_names(&world.dir), ["Save0003.sav"]);
        fs::remove_file(&target).unwrap();

        // The new file does not read back as written: it is removed again.
        let result = safe_write(&world.env(), &new_character(&slot, 3, &baseline, &payload), &mut |step| {
            if step == WriteStep::Confirm {
                fs::write(&target, b"scribbled over").unwrap();
            }
        });
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::ConfirmFailed);
        assert!(error.message.contains("so it was removed"), "{}", error.message);
        assert!(file_names(&world.dir).is_empty());
        assert!(world.backups.list().is_empty());
    }

    #[test]
    fn inventory_growth_check_counts_real_entries() {
        let baseline = sample_doc(3);
        let weapon = LevelFree::of(&baseline.inventory().into_iter().find(|e| e.is_weapon).unwrap()).unwrap();
        let mut grown = baseline.clone();
        grown.duplicate_with(crate::testutil::weapon_ref(), &savekit::FallbackGearClassifier, &mut || 9).unwrap();
        // A new Overpower record is a placeholder and does not count as an item.
        grown.set_op_level(2).unwrap();
        let one = Check::InventoryGrowth { added: vec![(weapon.clone(), 1)] };
        assert_eq!(one.run(&baseline, &grown), Ok(()));
        assert!(one.run(&baseline, &baseline).is_err());
        let two = Check::InventoryGrowth { added: vec![(weapon.clone(), 2)] };
        assert!(two.run(&baseline, &grown).is_err());
        // One more item than was asked for.
        let mut extra = grown.clone();
        extra.import_codes_with(&[crate::testutil::ITEM_CODE], false, &mut || 5).unwrap();
        assert!(one.run(&baseline, &extra).is_err());
        assert!(Check::ItemLevel(crate::testutil::weapon_ref(), 99).run(&baseline, &grown).is_err());
        assert!(Check::PlaythroughsAtLeast(2).run(&baseline, &grown).is_ok());
        assert!(Check::PlaythroughsAtLeast(1).run(&baseline, &baseline).is_err());
        assert!(Check::SlaughterdomeUnlocked.run(&baseline, &baseline).is_err());
        assert!(Check::BackpackAtLeast(13).run(&baseline, &baseline).is_err());
        assert!(Check::BankAtLeast(6).run(&baseline, &baseline).is_ok());
    }

    #[test]
    fn steps_are_numbered_in_order() {
        let numbers: Vec<u8> = WriteStep::ALL.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(WriteStep::ALL.iter().all(|s| !s.label().is_empty()));
    }
}
