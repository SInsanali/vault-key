//! [`Core`]: the entry point a front end talks to. It lists characters, opens sessions,
//! saves them through the safe write path, and manages test slots and backups.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use savekit::{CharacterKey, Expectation, FallbackGearClassifier, SaveDocument};

use crate::backup::{BackupInfo, BackupStore, BACKUPS_FOLDER_NAME};
use crate::error::{fail, from_guard, CoreError, ErrorKind, Result};
use crate::guards;
use crate::library::{self, CharacterEntry};
use crate::policy::{self, WriteAccess};
use crate::session::{CharacterSession, SharedClassifier};
use crate::settings::{self, Settings, SETTINGS_FILE_NAME};
use crate::slot::SlotId;
use crate::time::{self, Clock};
use crate::writer::{self, Check, GameStatus, ProcessLister, WriteEnv, WriteMode, WriteOutcome, WriteRequest, WriteStep};

/// Environment variable that moves Vault Key's own data folder (settings and backups).
pub const DATA_DIR_ENV: &str = "GK_DATA_DIR";
/// Name of Vault Key's folder under `%LOCALAPPDATA%`.
pub const APP_FOLDER_NAME: &str = "VaultKey";
/// The data folder's name before the app was renamed (it was called Golden Key until 0.9.0).
/// The app copies settings and backups from it once; see the app's `migrate_old_data`.
pub const OLD_APP_FOLDER_NAME: &str = "GoldenKey";
/// Largest save file Vault Key opens from outside the save folder. Real saves are under
/// 100 KB; this only stops a wrong pick (a video, say) from being read into memory.
pub const MAX_SAVE_FILE_BYTES: u64 = 4 << 20;
/// Folder under the backups folder where a file that an export replaced is kept.
const EXPORT_BACKUPS_FOLDER: &str = "exports";

/// Where things are and how the outside world is reached. Everything is injectable, so
/// tests run on temp folders with a fake process list and a fixed clock.
#[derive(Clone)]
pub struct CoreConfig {
    /// The game's SaveData folder (the one that holds the Steam-ID folders).
    pub save_data_root: PathBuf,
    /// Vault Key's own folder: `settings.txt` and `backups\` live here. Normally
    /// `%LOCALAPPDATA%\VaultKey`. Never inside a SaveData folder or the game install.
    pub data_dir: PathBuf,
    /// Other SaveData folders on this machine (the real ones, when `save_data_root` is an
    /// override). Vault Key's data must not be inside these either.
    pub other_save_roots: Vec<PathBuf>,
    /// Lists running processes, to tell whether the game is running.
    pub processes: ProcessLister,
    pub clock: Clock,
    /// Decides which items are gear, for Duplicate Item.
    pub classifier: SharedClassifier,
}

impl CoreConfig {
    /// The configuration for real use: the game's SaveData folder (or `GK_SAVEDATA_DIR`),
    /// `%LOCALAPPDATA%\VaultKey` (or `GK_DATA_DIR`), the live process list, the system
    /// clock, and the classifier that needs no game data (replace it once game data is
    /// loaded).
    pub fn live() -> Result<CoreConfig> {
        CoreConfig::from_env(&|key| std::env::var_os(key))
    }

    /// [`CoreConfig::live`] with the environment passed in.
    pub fn from_env(env: &dyn Fn(&str) -> Option<OsString>) -> Result<CoreConfig> {
        let save_data_root = guards::save_data_root(env).map_err(|e| from_guard(ErrorKind::SaveFolder, e))?;
        let data_dir = match env(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => match env("LOCALAPPDATA").filter(|v| !v.is_empty()) {
                Some(local) => Path::new(&local).join(APP_FOLDER_NAME),
                None => {
                    return fail(
                        ErrorKind::Config,
                        format!("Cannot tell where to keep Vault Key's settings and backups: LOCALAPPDATA is not set. Set {DATA_DIR_ENV} to a folder."),
                    )
                }
            },
        };
        let other_save_roots =
            guards::real_save_data_candidates(env).into_iter().filter(|root| *root != save_data_root).collect();
        Ok(CoreConfig {
            save_data_root,
            data_dir,
            other_save_roots,
            processes: writer::live_process_lister(),
            clock: time::system_clock(),
            classifier: Arc::new(FallbackGearClassifier),
        })
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join(SETTINGS_FILE_NAME)
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join(BACKUPS_FOLDER_NAME)
    }
}

/// What a save, a "save as new" or a restore did. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    /// The file that was written.
    pub slot_id: SlotId,
    pub path: String,
    /// The backup of what the file held before. `None` when a new file was created.
    pub backup: Option<BackupInfo>,
    /// True when a new character file was created rather than an existing one replaced.
    pub created_new: bool,
    /// Something that went wrong after the save itself succeeded, for the user to see.
    pub warning: Option<String>,
}

/// Said when a character opened from a file outside the save folder is to be saved in place.
pub const EXTERNAL_MESSAGE: &str =
    "This character was opened from a file outside your save folder, so there is no slot to save it into. Export it (back over that file, if you like), or add it to your characters.";

/// True when a path lies under a folder named `SaveData` inside one named `WillowGame`: a
/// Borderlands 2 save folder, wherever it is, known to the core or not.
fn looks_like_a_save_folder(path: &Path) -> bool {
    let parts: Vec<String> = guards::physical_path(path).components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    parts.windows(2).any(|pair| pair[0] == "willowgame" && pair[1] == "savedata")
}

/// What an export to a file did. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// The file that was written.
    pub path: String,
    pub file_name: String,
    /// The character as written.
    pub name: String,
    pub class_label: String,
    pub level: i64,
    pub save_game_id: i64,
    /// Where the file that was there before was kept, if one was replaced.
    pub replaced_backup: Option<String>,
}

/// The application core. One per running app; a front end keeps it (and the sessions it
/// opens) in its state and calls into it.
pub struct Core {
    save_data_root: PathBuf,
    data_dir: PathBuf,
    other_save_roots: Vec<PathBuf>,
    processes: ProcessLister,
    classifier: SharedClassifier,
    settings: Settings,
    backups: BackupStore,
    clock: Clock,
    progress: Option<Box<dyn FnMut(WriteStep) + Send>>,
    writes: u64,
}

impl Core {
    /// Checks the folders and loads the settings. Nothing in SaveData is read or written.
    pub fn new(config: CoreConfig) -> Result<Core> {
        if !config.data_dir.is_absolute() || !config.save_data_root.is_absolute() {
            return fail(ErrorKind::Config, "Vault Key's folders must be given as full paths.");
        }
        let mut save_roots = vec![config.save_data_root.clone()];
        save_roots.extend(config.other_save_roots.iter().cloned());
        if save_roots.iter().any(|root| guards::is_inside(&config.data_dir, root)) {
            return fail(
                ErrorKind::Config,
                format!(
                    "Vault Key's settings and backups can't be kept in {}: that is inside the game's save folder, which Steam Cloud syncs.",
                    config.data_dir.display()
                ),
            );
        }
        let in_steam_library = guards::physical_path(&config.data_dir)
            .components()
            .any(|part| part.as_os_str().to_string_lossy().eq_ignore_ascii_case("steamapps"));
        if in_steam_library {
            return fail(
                ErrorKind::Config,
                format!(
                    "Vault Key's settings and backups can't be kept in {}: that is inside a Steam library, and the game install is never written.",
                    config.data_dir.display()
                ),
            );
        }
        let mut settings = Settings::load(&config.settings_path())?;
        // Slots are identified by Steam ID and file name, so the settings and backups in
        // this data folder must be about this save folder and no other.
        let save_folder = settings::save_folder_key(&config.save_data_root);
        match settings.save_folder() {
            Some(theirs) if theirs != save_folder => {
                return fail(
                    ErrorKind::Config,
                    format!(
                        "The settings and backups in {} belong to a different save folder ({theirs}), not {}. Use a separate data folder for each save folder: set {DATA_DIR_ENV} to another folder. (If the game's save folder has really moved, delete the save-folder line from {}.)",
                        config.data_dir.display(),
                        config.save_data_root.display(),
                        config.settings_path().display()
                    ),
                )
            }
            Some(_) => {}
            None => settings.set_save_folder(save_folder),
        }
        let backups = BackupStore::new(config.backups_dir(), config.clock.clone());
        let clock = config.clock.clone();
        Ok(Core {
            save_data_root: config.save_data_root,
            data_dir: config.data_dir,
            other_save_roots: config.other_save_roots,
            processes: config.processes,
            classifier: config.classifier,
            settings,
            backups,
            clock,
            progress: None,
            writes: 0,
        })
    }

    pub fn save_data_root(&self) -> &Path {
        &self.save_data_root
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join(SETTINGS_FILE_NAME)
    }

    pub fn backups_dir(&self) -> &Path {
        self.backups.root()
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Sets what is told each step of a write as it starts (for a progress display), or
    /// clears it with `None`.
    pub fn set_progress_listener(&mut self, listener: Option<Box<dyn FnMut(WriteStep) + Send>>) {
        self.progress = listener;
    }

    /// The classifier that sessions opened from now on use.
    pub fn set_classifier(&mut self, classifier: SharedClassifier) {
        self.classifier = classifier;
    }

    /// Whether the game is running. Saving is refused unless this is `NotRunning`.
    pub fn game_status(&self) -> GameStatus {
        writer::game_status(&*self.processes)
    }

    // Characters

    /// Every save file in the SaveData folder. A file that cannot be decoded is listed with
    /// its problem. An error only if the SaveData folder itself cannot be listed.
    ///
    /// Also forgets write permission for files that are gone, so that a character the game
    /// later creates under the same file name starts read-only.
    pub fn list_characters(&mut self) -> Result<Vec<CharacterEntry>> {
        let steam_ids = library::steam_ids(&self.save_data_root)?;
        let mut recorded = self.settings.created_slots();
        recorded.extend(self.settings.test_slots());
        let mut forgot = false;
        for slot in recorded {
            let gone = if steam_ids.iter().any(|id| id == slot.steam_id()) {
                // If the folder cannot be listed, nothing is known, and nothing is forgotten.
                library::save_file_names(&self.save_data_root.join(slot.steam_id()))
                    .is_some_and(|names| !names.iter().any(|name| name.eq_ignore_ascii_case(slot.file_name())))
            } else {
                true
            };
            if gone {
                forgot |= self.settings.forget(&slot);
            }
        }
        if forgot {
            // If this fails the stale entries are still on disk, and are dropped again next time.
            let _ = self.settings.save(&self.settings_path());
        }
        library::scan(&self.save_data_root, &self.settings)
    }

    /// The Steam-ID folders in the SaveData folder.
    pub fn steam_ids(&self) -> Result<Vec<String>> {
        library::steam_ids(&self.save_data_root)
    }

    /// Slots (2 to 9) in a Steam-ID folder that hold no file, where "Save as a new
    /// character" can put one.
    pub fn free_slots(&self, steam_id: &str) -> Result<Vec<u32>> {
        let probe = SlotId::for_slot(steam_id, guards::MIN_SLOT)?;
        let (dir, _) = writer::resolve_target(&self.save_data_root, &probe)?;
        Ok((guards::MIN_SLOT..=guards::MAX_SLOT)
            .filter(|slot| fs::symlink_metadata(dir.join(guards::slot_file_name(*slot))).is_err())
            .collect())
    }

    /// Whether Vault Key may edit this file in place, and why or why not.
    pub fn access(&self, slot_id: &SlotId) -> WriteAccess {
        let document = self.document_on_disk(slot_id);
        policy::access_for(&self.settings, slot_id, document.as_ref())
    }

    fn document_on_disk(&self, slot_id: &SlotId) -> Option<SaveDocument> {
        let (_, target) = writer::resolve_target(&self.save_data_root, slot_id).ok()?;
        SaveDocument::from_file_bytes(&fs::read(target).ok()?).ok()
    }

    /// Protects the character that is in this slot now: Vault Key will never write its file
    /// again, whatever else is set, and the slot cannot be marked as a test slot while it is
    /// there. A copy made with "Save as a new character" is not protected. This is what the
    /// app's Lock button does; [`Core::unlock`] undoes it.
    pub fn protect(&mut self, slot_id: &SlotId) -> Result<()> {
        let Some(document) = self.document_on_disk(slot_id) else {
            return fail(ErrorKind::InvalidTarget, format!("{} cannot be read, so it cannot be protected.", slot_id.file_name()));
        };
        let before = self.settings.clone();
        let label = format!("{} ({}, level {}, save-game ID {})", document.name(), library::class_label(&document.class_path()), document.level(), document.save_game_id());
        self.settings.protect(&policy::protection_key(&document), &label);
        // A test-slot mark on that slot would mean nothing now; take it off.
        self.settings.unmark_test_slot(slot_id);
        self.keep_settings(before)
    }

    /// Sets the color theme (one of [`crate::settings::THEMES`]) and writes the settings.
    pub fn set_theme(&mut self, theme: &str) -> Result<()> {
        let before = self.settings.clone();
        if !self.settings.set_theme(theme) {
            return fail(ErrorKind::InvalidValue, format!("There is no color theme called {theme}."));
        }
        self.keep_settings(before)
    }

    /// Locks the character in this slot: the same as [`Core::protect`].
    pub fn lock(&mut self, slot_id: &SlotId) -> Result<()> {
        self.protect(slot_id)
    }

    /// Unlocks the character that is in this slot now, so that it can be edited in place:
    /// takes off its protection, if it has one, and marks the slot as a test slot for it
    /// (unless Vault Key created the file, which is writable already). This is the user's
    /// explicit decision about that one character; saving still backs the file up first.
    pub fn unlock(&mut self, slot_id: &SlotId) -> Result<()> {
        if slot_id.slot().is_none() {
            return fail(ErrorKind::InvalidTarget, policy::NOT_A_SLOT_MESSAGE);
        }
        let Some(document) = self.document_on_disk(slot_id) else {
            return fail(ErrorKind::InvalidTarget, format!("{} cannot be read, so it cannot be unlocked.", slot_id.file_name()));
        };
        let before = self.settings.clone();
        self.settings.unprotect(&policy::protection_key(&document));
        if !policy::access_for(&self.settings, slot_id, Some(&document)).is_writable() {
            self.settings.mark_test_slot(slot_id, &policy::character_mark(&document));
        }
        self.keep_settings(before)
    }

    /// The mark of the character that is in a slot's file now; `None` if there is no file or
    /// it cannot be read.
    fn character_on_disk(&self, slot_id: &SlotId) -> Option<String> {
        let (_, target) = writer::resolve_target(&self.save_data_root, slot_id).ok()?;
        let document = SaveDocument::from_file_bytes(&fs::read(target).ok()?).ok()?;
        Some(policy::character_mark(&document))
    }

    /// Marks an existing character file's slot as a test slot, which makes it editable in
    /// place. This is the user's explicit decision; nothing else makes an existing file
    /// writable. The mark is for the character that is in the slot now: if a different
    /// character turns up under the same file name later, the mark does not cover it.
    pub fn mark_test_slot(&mut self, slot_id: &SlotId) -> Result<()> {
        if slot_id.slot().is_none() {
            return fail(ErrorKind::InvalidTarget, policy::NOT_A_SLOT_MESSAGE);
        }
        let (_, target) = writer::resolve_target(&self.save_data_root, slot_id)?;
        if !target.is_file() {
            return fail(
                ErrorKind::InvalidTarget,
                format!("There is no {} to mark as a test slot.", slot_id.file_name()),
            );
        }
        if self.document_on_disk(slot_id).is_some_and(|d| policy::is_protected(&self.settings, &d)) {
            return fail(ErrorKind::ReadOnly, policy::PROTECTED_MESSAGE);
        }
        let Some(character) = self.character_on_disk(slot_id) else {
            return fail(
                ErrorKind::InvalidTarget,
                format!("{} cannot be read, so it cannot be marked as a test slot.", slot_id.file_name()),
            );
        };
        let before = self.settings.clone();
        self.settings.mark_test_slot(slot_id, &character);
        self.keep_settings(before)
    }

    /// Takes the test-slot mark off a slot. The file becomes read-only again (unless Vault
    /// Key created it).
    pub fn unmark_test_slot(&mut self, slot_id: &SlotId) -> Result<()> {
        let before = self.settings.clone();
        if !self.settings.unmark_test_slot(slot_id) {
            return Ok(());
        }
        self.keep_settings(before)
    }

    /// Writes the settings file; if that fails, goes back to `before` so that memory and
    /// disk agree.
    fn keep_settings(&mut self, before: Settings) -> Result<()> {
        if let Err(e) = self.settings.save(&self.settings_path()) {
            self.settings = before;
            return Err(e);
        }
        Ok(())
    }

    /// Opens a character for editing. Reading is always allowed; whether it can be saved in
    /// place is [`Core::access`].
    pub fn open(&self, slot_id: &SlotId) -> Result<CharacterSession> {
        let (_, target) = writer::resolve_target(&self.save_data_root, slot_id)?;
        let bytes = fs::read(&target).map_err(|e| {
            CoreError::new(ErrorKind::Unreadable, format!("{} can't be opened: {e}", slot_id.file_name()))
        })?;
        CharacterSession::from_bytes(slot_id.clone(), bytes, self.classifier.clone())
    }

    /// Opens a save file from anywhere ("Open a save file…"). A file in this save folder's
    /// own Steam-ID folders is opened as that slot, like any listed character; anything else
    /// is opened as an external character, which Vault Key never writes to. Reading only.
    pub fn open_file(&self, path: &Path) -> Result<CharacterSession> {
        if !path.is_absolute() {
            return fail(ErrorKind::InvalidTarget, "Choose a save file by its full path.");
        }
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        if let Some(slot_id) = self.slot_for_path(path) {
            return self.open(&slot_id);
        }
        let metadata = fs::metadata(path)
            .map_err(|e| CoreError::new(ErrorKind::Unreadable, format!("{name} can't be opened: {e}")))?;
        if !metadata.is_file() {
            return fail(ErrorKind::Unreadable, format!("{name} is not a file."));
        }
        if metadata.len() > MAX_SAVE_FILE_BYTES {
            return fail(
                ErrorKind::Unreadable,
                format!("{name} is {} MB, far bigger than any save, so it was not opened.", metadata.len() >> 20),
            );
        }
        let mut bytes = Vec::new();
        {
            use std::io::Read as _;
            let file = fs::File::open(path).map_err(|e| CoreError::new(ErrorKind::Unreadable, format!("{name} can't be opened: {e}")))?;
            file.take(MAX_SAVE_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| CoreError::new(ErrorKind::Unreadable, format!("{name} can't be read: {e}")))?;
        }
        if bytes.len() as u64 > MAX_SAVE_FILE_BYTES {
            return fail(ErrorKind::Unreadable, format!("{name} is far bigger than any save, so it was not opened."));
        }
        CharacterSession::from_external(path.to_path_buf(), bytes, self.classifier.clone())
    }

    /// The slot a path names, when it is a save file directly inside one of this save
    /// folder's Steam-ID folders.
    fn slot_for_path(&self, path: &Path) -> Option<SlotId> {
        if !guards::is_inside(path, &self.save_data_root) {
            return None;
        }
        let file_name = path.file_name()?.to_str()?;
        let steam_dir = path.parent()?;
        let steam_id = steam_dir.file_name()?.to_str()?;
        let root = steam_dir.parent()?;
        if !guards::is_inside(root, &self.save_data_root) || !guards::is_inside(&self.save_data_root, root) {
            return None;
        }
        SlotId::new(steam_id, file_name).ok()
    }

    /// Why a file may not be written by an export, or `None` when it may. Exports never go
    /// into a save folder (Save and "Save as a new character" do that, with the game check
    /// and the ID guards), the game install, or Vault Key's own folder. The file a
    /// character was opened from may be exported over, like any other file outside those.
    fn export_refusal(&self, target: &Path) -> Option<String> {
        let in_save_folder = std::iter::once(&self.save_data_root)
            .chain(self.other_save_roots.iter())
            .any(|root| guards::is_inside(target, root))
            || looks_like_a_save_folder(target);
        if in_save_folder {
            return Some(
                "That is inside the game's save folder. To put a character there, use Save or \"Save to new slot\", which check that the game is closed and that no two saves share an ID.".to_string(),
            );
        }
        let physical = guards::physical_path(target);
        if physical.components().any(|part| part.as_os_str().to_string_lossy().eq_ignore_ascii_case("steamapps")) {
            return Some("That is inside a Steam library. Vault Key never writes in the game's folders.".to_string());
        }
        if guards::is_inside(target, &self.data_dir) {
            return Some("That is Vault Key's own folder (settings and backups). Choose another folder, such as Documents.".to_string());
        }
        None
    }

    /// Writes the character, as it would be after its pending changes, to a file of the
    /// user's choosing, to give to a friend or keep. Everything is kept as it is, the
    /// save-game ID too. The file is written next to its target, read back and verified,
    /// then moved into place in one step; a file it replaces is kept in Vault Key's
    /// backups folder first. The session is not changed. The target may be the file an
    /// external character was opened from: the user chose it, and the old file is kept.
    pub fn export(&mut self, session: &CharacterSession, target: &Path) -> Result<ExportReport> {
        if !target.is_absolute() {
            return fail(ErrorKind::InvalidTarget, "Choose where to export by a full path.");
        }
        let Some(file_name) = target.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return fail(ErrorKind::InvalidTarget, "Choose a file name to export to.");
        };
        if let Some(why) = self.export_refusal(target) {
            return fail(ErrorKind::InvalidTarget, format!("The character was not exported there. {why}"));
        }
        let Some(dir) = target.parent().filter(|dir| dir.is_dir()) else {
            return fail(ErrorKind::InvalidTarget, "The folder to export into does not exist.");
        };
        let existing = match fs::symlink_metadata(target) {
            Ok(meta) if meta.is_file() => Some(fs::read(target).map_err(|e| {
                CoreError::new(ErrorKind::BackupFailed, format!("{file_name} is there already and can't be read to keep a copy ({e}), so nothing was written."))
            })?),
            Ok(_) => return fail(ErrorKind::InvalidTarget, format!("{file_name} is not a file, so nothing was written there.")),
            Err(_) => None,
        };

        let document = session.working().clone();
        let payload = Self::encode(&document)?;
        let (expectations, mut checks) = session.verification();
        checks.retain(|check| !matches!(check, Check::Equals(_)));
        checks.push(Check::Equals(document.clone()));

        // Keep what is there now, in Vault Key's backups folder.
        let replaced_backup = match &existing {
            Some(old) => Some(self.keep_replaced_export(&file_name, old)?),
            None => None,
        };

        self.writes += 1;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let temp_path = dir.join(format!(".gk-export-{}-{nanos}-{}.tmp", std::process::id(), self.writes));
        let not_written = |e: std::io::Error| {
            CoreError::new(ErrorKind::WriteFailed, format!("Could not write the export, so nothing was changed ({e})."))
        };
        let result = (|| {
            {
                use std::io::Write as _;
                let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).map_err(not_written)?;
                file.write_all(&payload).and_then(|()| file.sync_all()).map_err(not_written)?;
            }
            let unverified = |why: String| {
                CoreError::new(ErrorKind::VerificationFailed, format!("The exported file did not pass verification, so it was not kept ({why})."))
            };
            let back = fs::read(&temp_path).map_err(|e| unverified(format!("cannot read it back: {e}")))?;
            if back != payload {
                return Err(unverified("it read back differently".to_string()));
            }
            savekit::verify(session.original(), &back, &expectations).map_err(|e| unverified(e.to_string()))?;
            let written = SaveDocument::from_file_bytes(&back).map_err(|e| unverified(e.to_string()))?;
            for check in &checks {
                check.run(session.original(), &written).map_err(unverified)?;
            }
            fs::rename(&temp_path, target).map_err(|e| {
                CoreError::new(ErrorKind::ReplaceFailed, format!("Could not put the export in place, so nothing was changed ({e})."))
            })?;
            if !fs::read(target).is_ok_and(|on_disk| on_disk == payload) {
                return fail(ErrorKind::ConfirmFailed, format!("{file_name} did not read back as the export. Export again."));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result?;
        Ok(ExportReport {
            path: target.display().to_string(),
            file_name,
            name: document.name(),
            class_label: library::class_label(&document.class_path()),
            level: document.level(),
            save_game_id: document.save_game_id(),
            replaced_backup,
        })
    }

    /// Copies a file an export is about to replace into `backups\exports`, under a name of
    /// its own. Returns where it went.
    fn keep_replaced_export(&self, file_name: &str, bytes: &[u8]) -> Result<String> {
        let dir = self.backups.root().join(EXPORT_BACKUPS_FOLDER);
        let failed = |e: std::io::Error| {
            CoreError::new(ErrorKind::BackupFailed, format!("The file being replaced could not be backed up ({e}), so nothing was written."))
        };
        fs::create_dir_all(&dir).map_err(failed)?;
        let stamp = time::backup_stamp(time::unix_seconds((self.clock)()));
        let safe: String = file_name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
        for n in 1..1000 {
            let suffix = if n == 1 { String::new() } else { format!("-{n}") };
            let path = dir.join(format!("{stamp}{suffix} {safe}"));
            match guards::write_new_file(&path, bytes) {
                Ok(()) => return Ok(path.display().to_string()),
                Err(_) if path.exists() => continue,
                Err(e) => return fail(ErrorKind::BackupFailed, format!("The file being replaced could not be backed up ({}), so nothing was written.", e.0)),
            }
        }
        fail(ErrorKind::BackupFailed, "The file being replaced could not be backed up, so nothing was written.")
    }

    // Saving

    fn write(&mut self, request: &WriteRequest) -> Result<WriteOutcome> {
        self.writes += 1;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let unique = format!("{}-{nanos}-{}", std::process::id(), self.writes);
        let env = WriteEnv {
            save_root: &self.save_data_root,
            other_save_roots: &self.other_save_roots,
            settings: &self.settings,
            backups: &self.backups,
            processes: &*self.processes,
            unique: &unique,
        };
        let mut silent = |_: WriteStep| {};
        let on_step: &mut dyn FnMut(WriteStep) = match self.progress.as_mut() {
            Some(listener) => &mut **listener,
            None => &mut silent,
        };
        let outcome = writer::safe_write(&env, request, on_step)?;
        // A test-slot mark is for one character. Vault Key itself has just put this content
        // in the slot (a rename, or an older backup), so the mark follows it. If that cannot
        // be recorded the slot is simply read-only until it is marked again.
        if matches!(request.mode, WriteMode::Replace { .. }) && self.settings.is_test_slot(request.slot_id) {
            if let Ok(written) = SaveDocument::from_file_bytes(request.payload) {
                let character = policy::character_mark(&written);
                if self.settings.test_slot_character(request.slot_id) != Some(character.as_str()) {
                    let before = self.settings.clone();
                    self.settings.mark_test_slot(request.slot_id, &character);
                    let _ = self.keep_settings(before);
                }
            }
        }
        Ok(outcome)
    }

    /// Records a file this app has just created, so that it is writable from now on.
    fn record_created(&mut self, slot_id: &SlotId) -> Option<String> {
        self.settings.record_created(slot_id);
        self.settings.save(&self.settings_path()).err().map(|e| {
            format!(
                "{} was written, but Vault Key could not record that it created it, so after a restart it will be read-only until you mark it as a test slot. ({})",
                slot_id.file_name(),
                e.message
            )
        })
    }

    fn encode(document: &SaveDocument) -> Result<Vec<u8>> {
        document.to_file_bytes().map_err(|e| {
            CoreError::new(ErrorKind::VerificationFailed, format!("The edited character can't be written as a save file ({e})."))
        })
    }

    /// Saves the session's pending changes into its own file, through the safe write path:
    /// policy, game check, unchanged-on-disk check, backup, temp file, verification, swap,
    /// confirmation. On success the session continues from the saved state with nothing
    /// pending. On any failure the file is as it was and the session is untouched.
    pub fn save_in_place(&mut self, session: &mut CharacterSession) -> Result<SaveReport> {
        if !session.has_changes() {
            return fail(ErrorKind::NothingToSave, "There are no changes to save.");
        }
        let Some(slot_id) = session.slot_id().cloned() else {
            return fail(ErrorKind::ReadOnly, EXTERNAL_MESSAGE);
        };
        let payload = Self::encode(session.working())?;
        let (expectations, checks) = session.verification();
        let count = session.pending_count();
        let reason = format!("Before saving {count} change{}", if count == 1 { "" } else { "s" });
        let outcome = self.write(&WriteRequest {
            slot_id: &slot_id,
            mode: WriteMode::Replace { expected_on_disk: session.original_bytes() },
            payload: &payload,
            baseline: session.original(),
            expectations: &expectations,
            checks: &checks,
            reason: &reason,
        })?;
        let saved = session.working().clone();
        session.rebase(payload, saved);
        Ok(SaveReport {
            slot_id,
            path: outcome.path.display().to_string(),
            backup: outcome.backup,
            created_new: false,
            warning: None,
        })
    }

    /// Writes the session's working character into a free slot (2 to 9) of the same
    /// Steam-ID folder as a new character, with its save-game ID set to the slot. Nothing
    /// can be overwritten, and no two saves can end up with the same ID. The new file is
    /// recorded as created by Vault Key, so it can be edited in place afterwards. The
    /// session itself is not changed.
    ///
    /// A character opened from a file outside the save folder goes into the only Steam-ID
    /// folder there is; when there are several, use [`Core::save_as_new_in`].
    pub fn save_as_new(&mut self, session: &CharacterSession, slot: u32) -> Result<SaveReport> {
        let steam_id = match session.slot_id() {
            Some(slot_id) => slot_id.steam_id().to_string(),
            None => match self.steam_ids()?.as_slice() {
                [only] => only.clone(),
                [] => return fail(ErrorKind::SaveFolder, "The save folder has no Steam-ID folder to put the character in. Start the game once first."),
                _ => return fail(ErrorKind::InvalidTarget, "There is more than one Steam account's folder in the save folder. Choose which one the character goes into."),
            },
        };
        self.save_as_new_in(session, &steam_id, slot)
    }

    /// [`Core::save_as_new`] into a given Steam-ID folder of the save folder.
    pub fn save_as_new_in(&mut self, session: &CharacterSession, steam_id: &str, slot: u32) -> Result<SaveReport> {
        if !self.steam_ids()?.iter().any(|id| id == steam_id) {
            return fail(ErrorKind::InvalidTarget, format!("There is no folder for Steam ID {steam_id} in the save folder."));
        }
        let slot_id = SlotId::for_slot(steam_id, slot)?;
        let mut document = session.working().clone();
        // Slot rules are the write path's to enforce; a slot of 0 just cannot be an ID.
        if document.set_save_game_id(i64::from(slot)).is_err() {
            return fail(ErrorKind::InvalidTarget, format!("{slot} is not a slot Vault Key can save to."));
        }
        let payload = Self::encode(&document)?;
        let (mut expectations, mut checks) = session.verification();
        expectations.push(Expectation::CharacterValue(CharacterKey::SaveGameId, i64::from(slot)));
        // The written file must be the working character with the new ID, and nothing else.
        checks.retain(|check| !matches!(check, Check::Equals(_)));
        checks.push(Check::Equals(document));
        let outcome = self.write(&WriteRequest {
            slot_id: &slot_id,
            mode: WriteMode::CreateNew { slot },
            payload: &payload,
            baseline: session.original(),
            expectations: &expectations,
            checks: &checks,
            reason: "",
        })?;
        let warning = self.record_created(&slot_id);
        Ok(SaveReport { slot_id, path: outcome.path.display().to_string(), backup: None, created_new: true, warning })
    }

    // Backups

    /// Every backup, newest first, with what each is a backup of and when it was taken.
    pub fn list_backups(&self) -> Vec<BackupInfo> {
        self.backups.list()
    }

    /// Backups of one character file, newest first.
    pub fn list_backups_for(&self, slot_id: &SlotId) -> Vec<BackupInfo> {
        self.backups.list_for(slot_id)
    }

    /// Puts a backup back over the file it was taken from, through the same safe write
    /// path as a save: the file must be writable by policy, the game must be closed, and
    /// what is there now is backed up first. If the file is gone and its slot is free, the
    /// backup is placed as a new file under the rules for new characters.
    ///
    /// A session open on that file is out of date afterwards: reopen it.
    pub fn restore_backup(&mut self, backup_id: &str) -> Result<SaveReport> {
        let info = self.backups.find(backup_id)?;
        let payload = self.backups.read(&info)?;
        let document = SaveDocument::from_file_bytes(&payload).map_err(|e| {
            CoreError::new(
                ErrorKind::BadBackup,
                format!("This backup is not a readable save, so it will not be restored. {}", library::unreadable_reason(&e)),
            )
        })?;
        let slot_id = info.slot_id.clone();
        let (_, target) = writer::resolve_target(&self.save_data_root, &slot_id)?;
        let reason = "Before restoring an earlier backup".to_string();
        // The restored file must be the backup, bit for bit.
        let expectations = [Expectation::Unchanged];
        let checks = [Check::Equals(document.clone())];
        match fs::read(&target) {
            Ok(current) => {
                let outcome = self.write(&WriteRequest {
                    slot_id: &slot_id,
                    mode: WriteMode::Replace { expected_on_disk: &current },
                    payload: &payload,
                    baseline: &document,
                    expectations: &expectations,
                    checks: &checks,
                    reason: &reason,
                })?;
                Ok(SaveReport {
                    path: outcome.path.display().to_string(),
                    backup: outcome.backup,
                    created_new: false,
                    warning: None,
                    slot_id,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(slot) = slot_id.slot() else {
                    return fail(ErrorKind::InvalidTarget, policy::NOT_A_SLOT_MESSAGE);
                };
                let outcome = self.write(&WriteRequest {
                    slot_id: &slot_id,
                    mode: WriteMode::CreateNew { slot },
                    payload: &payload,
                    baseline: &document,
                    expectations: &expectations,
                    checks: &checks,
                    reason: &reason,
                })?;
                let warning = self.record_created(&slot_id);
                Ok(SaveReport {
                    path: outcome.path.display().to_string(),
                    backup: None,
                    created_new: true,
                    warning,
                    slot_id,
                })
            }
            Err(e) => fail(
                ErrorKind::ChangedOnDisk,
                format!("{} can't be read right now ({e}), so the backup was not restored.", slot_id.file_name()),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::Edit;
    use crate::policy::WritableBecause;
    use crate::testutil::{file_names, sample_bytes, weapon_ref, FakeProcesses, TempDir, STEAM_ID};
    use savekit::Currency;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    const NOW: u64 = 1_794_194_100; // 2026-11-09 03:15:00 UTC

    /// A Core on temp folders: `<temp>/SaveData/<steam id>` and `<temp>/VaultKey`.
    struct World {
        temp: TempDir,
        dir: PathBuf,
        processes: FakeProcesses,
        now: Arc<AtomicU64>,
        core: Core,
    }

    impl World {
        fn new() -> World {
            let temp = TempDir::new();
            let dir = temp.path().join("SaveData").join(STEAM_ID);
            fs::create_dir_all(&dir).unwrap();
            let processes = FakeProcesses::new();
            let now = Arc::new(AtomicU64::new(NOW));
            let core = Core::new(Self::config(&temp, &processes, &now)).unwrap();
            World { temp, dir, processes, now, core }
        }

        fn config(temp: &TempDir, processes: &FakeProcesses, now: &Arc<AtomicU64>) -> CoreConfig {
            let now = now.clone();
            CoreConfig {
                save_data_root: temp.path().join("SaveData"),
                data_dir: temp.path().join("VaultKey"),
                other_save_roots: Vec::new(),
                processes: processes.lister(),
                clock: Arc::new(move || {
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(now.load(Ordering::SeqCst))
                }),
                classifier: Arc::new(FallbackGearClassifier),
            }
        }

        /// The app is closed and started again: settings come from disk.
        fn restart(&mut self) {
            self.core = Core::new(Self::config(&self.temp, &self.processes, &self.now)).unwrap();
        }

        fn put(&self, file: &str, save_game_id: i64) -> Vec<u8> {
            let bytes = sample_bytes(save_game_id);
            fs::write(self.dir.join(file), &bytes).unwrap();
            bytes
        }

        fn read(&self, file: &str) -> Vec<u8> {
            fs::read(self.dir.join(file)).unwrap()
        }

        fn slot(&self, file: &str) -> SlotId {
            SlotId::new(STEAM_ID, file).unwrap()
        }
    }

    #[test]
    fn folders_are_checked_before_anything_is_touched() {
        let temp = TempDir::new();
        let processes = FakeProcesses::new();
        let now = Arc::new(AtomicU64::new(NOW));
        let good = World::config(&temp, &processes, &now);
        fs::create_dir_all(good.save_data_root.join(STEAM_ID)).unwrap();

        let refused = |config: CoreConfig| Core::new(config).err().map(|e| (e.kind, e.message));
        // Backups and settings inside the save folder would be synced by Steam Cloud.
        let mut bad = good.clone();
        bad.data_dir = good.save_data_root.join("VaultKey");
        assert!(refused(bad).unwrap().1.contains("inside the game's save folder"));
        let mut bad = good.clone();
        bad.data_dir = good.save_data_root.join(STEAM_ID).join("deep").join("er");
        assert_eq!(refused(bad).unwrap().0, ErrorKind::Config);
        // The same goes for any other save folder on the machine.
        let mut bad = good.clone();
        bad.other_save_roots = vec![temp.path().join("RealSaveData")];
        bad.data_dir = temp.path().join("RealSaveData").join("x");
        assert_eq!(refused(bad).unwrap().0, ErrorKind::Config);
        // Never inside a Steam library (the game install).
        let mut bad = good.clone();
        bad.data_dir = temp.path().join("SteamApps").join("common").join("Borderlands 2").join("VaultKey");
        assert!(refused(bad).unwrap().1.contains("Steam library"));
        // Relative paths depend on where the app was started.
        let mut bad = good.clone();
        bad.data_dir = PathBuf::from("VaultKey");
        assert_eq!(refused(bad).unwrap().0, ErrorKind::Config);

        let core = Core::new(good).unwrap();
        assert_eq!(core.backups_dir(), temp.path().join("VaultKey").join("backups"));
        assert_eq!(core.settings_path(), temp.path().join("VaultKey").join("settings.txt"));
        // Creating the Core wrote nothing anywhere.
        assert_eq!(file_names(temp.path()), ["SaveData"]);
    }

    #[test]
    fn settings_and_backups_made_for_one_save_folder_are_not_applied_to_another() {
        // A sandbox save folder, where slot 3 is marked as a test slot.
        let mut world = World::new();
        world.put("Save0003.sav", 3);
        world.core.mark_test_slot(&world.slot("Save0003.sav")).unwrap();
        // The same folder again, spelled differently: fine.
        let mut config = World::config(&world.temp, &world.processes, &world.now);
        config.save_data_root = PathBuf::from(config.save_data_root.to_string_lossy().to_uppercase()).join("x").join("..");
        assert!(Core::new(config).unwrap().access(&world.slot("Save0003.sav")).is_writable());

        // Another save folder with the same Steam ID and file name (the real one, say),
        // pointed at the same data folder: the mark must not carry over.
        let other_root = world.temp.path().join("OtherSaveData");
        fs::create_dir_all(other_root.join(STEAM_ID)).unwrap();
        fs::write(other_root.join(STEAM_ID).join("Save0003.sav"), sample_bytes(3)).unwrap();
        let mut config = World::config(&world.temp, &world.processes, &world.now);
        config.save_data_root = other_root.clone();
        let error = Core::new(config.clone()).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Config);
        assert!(error.message.contains("belong to a different save folder"), "{}", error.message);
        assert!(error.message.contains("GK_DATA_DIR"), "{}", error.message);

        // With its own data folder it starts, with nothing writable.
        config.data_dir = world.temp.path().join("OtherVaultKey");
        let mut other = Core::new(config).unwrap();
        assert!(other.list_characters().unwrap().iter().all(|c| !c.writable));
        assert!(other.list_backups().is_empty());
    }

    #[test]
    fn config_from_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| pairs.iter().find(|(name, _)| *name == key).map(|(_, value)| OsString::from(value))
        };
        let config = CoreConfig::from_env(&env(&[
            ("GK_SAVEDATA_DIR", "X:\\sandbox\\SaveData"),
            ("LOCALAPPDATA", "C:\\Users\\me\\AppData\\Local"),
            ("USERPROFILE", "C:\\Users\\me"),
        ]))
        .unwrap();
        assert_eq!(config.save_data_root, PathBuf::from("X:\\sandbox\\SaveData"));
        assert_eq!(config.data_dir, Path::new("C:\\Users\\me\\AppData\\Local").join("VaultKey"));
        assert_eq!(config.settings_path(), config.data_dir.join("settings.txt"));
        assert_eq!(config.backups_dir(), config.data_dir.join("backups"));
        // The real save folder is still known, so that nothing of ours is ever put in it.
        assert_eq!(config.other_save_roots.len(), 1);
        assert!(config.other_save_roots[0].ends_with("Documents/My Games/Borderlands 2/WillowGame/SaveData"));

        let config = CoreConfig::from_env(&env(&[
            ("GK_SAVEDATA_DIR", "X:\\sandbox\\SaveData"),
            ("GK_DATA_DIR", "X:\\sandbox\\data"),
            ("LOCALAPPDATA", "C:\\Users\\me\\AppData\\Local"),
        ]))
        .unwrap();
        assert_eq!(config.data_dir, PathBuf::from("X:\\sandbox\\data"));

        let error = CoreConfig::from_env(&env(&[("GK_SAVEDATA_DIR", "X:\\sandbox\\SaveData")])).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Config);
        let error = CoreConfig::from_env(&env(&[("USERPROFILE", "Q:\\no-such-profile-gk"), ("LOCALAPPDATA", "Q:\\x")])).err().unwrap();
        assert_eq!(error.kind, ErrorKind::SaveFolder);
    }

    #[test]
    fn lists_characters_with_their_access() {
        let mut world = World::new();
        world.put("Save0001.sav", 1);
        world.put("Save0002.sav", 2);
        fs::write(world.dir.join("Save0004.sav"), b"junk").unwrap();
        let list = world.core.list_characters().unwrap();
        assert_eq!(list.len(), 3);
        assert!(list.iter().all(|c| !c.writable), "nothing is writable by default");
        assert_eq!(list[0].access, WriteAccess::ReadOnly(policy::READ_ONLY_MESSAGE.to_string()));
        assert_eq!(list[1].access, WriteAccess::ReadOnly(policy::READ_ONLY_MESSAGE.to_string()));
        assert!(list[2].problem.is_some());
        assert_eq!(world.core.steam_ids().unwrap(), [STEAM_ID]);
        assert_eq!(world.core.free_slots(STEAM_ID).unwrap(), [3, 5, 6, 7, 8, 9]);
        assert_eq!(world.core.free_slots("111").unwrap_err().kind, ErrorKind::InvalidTarget);
        assert_eq!(world.core.game_status(), GameStatus::NotRunning);
        world.processes.running.store(true, Ordering::SeqCst);
        assert_eq!(world.core.game_status(), GameStatus::Running);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        assert!(matches!(world.core.game_status(), GameStatus::Unknown(_)));
    }

    #[test]
    fn an_existing_file_is_read_only_by_every_route_until_marked() {
        let mut world = World::new();
        let original = world.put("Save0002.sav", 2);
        let slot = world.slot("Save0002.sav");
        assert!(!world.core.access(&slot).is_writable());

        // Route 1: save in place.
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        let error = world.core.save_in_place(&mut session).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::ReadOnly, policy::READ_ONLY_MESSAGE));
        assert_eq!(session.pending_count(), 1, "the session keeps its changes after a refusal");

        // Route 2: restore. Make a real backup of this very file by marking, saving and
        // unmarking; then restoring it is refused like any other write.
        world.core.mark_test_slot(&slot).unwrap();
        let report = world.core.save_in_place(&mut session).unwrap();
        let backup = report.backup.unwrap();
        let edited = world.read("Save0002.sav");
        assert_ne!(edited, original);
        world.core.unmark_test_slot(&slot).unwrap();
        let error = world.core.restore_backup(&backup.id).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::ReadOnly, policy::READ_ONLY_MESSAGE));

        // Route 3: "save as new" aimed at the existing file's slot.
        let error = world.core.save_as_new(&session, 2).unwrap_err();
        assert_eq!(error.kind, ErrorKind::SlotTaken);

        assert_eq!(world.read("Save0002.sav"), edited, "byte-identical after every refusal");
        assert_eq!(file_names(&world.dir), ["Save0002.sav"]);
        assert_eq!(world.core.list_backups().len(), 1, "refused writes make no backups");
    }

    #[test]
    fn test_slot_marks_are_explicit_and_persistent() {
        let mut world = World::new();
        world.put("Save0001.sav", 1);
        world.put("Save0002.sav", 2);
        world.put("Save0002 - Copy.sav", 2);
        let slot = world.slot("Save0002.sav");

        assert_eq!(world.core.mark_test_slot(&world.slot("Save0002 - Copy.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
        assert_eq!(world.core.mark_test_slot(&world.slot("Save0005.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
        assert!(world.core.settings().test_slots().is_empty());
        assert!(!world.core.settings_path().exists());

        world.core.mark_test_slot(&world.slot("SAVE0002.sav")).unwrap();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));
        world.restart();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));
        let list = world.core.list_characters().unwrap();
        assert_eq!(list.iter().map(|c| c.writable).collect::<Vec<_>>(), [false, true, false]);

        world.core.unmark_test_slot(&slot).unwrap();
        world.core.unmark_test_slot(&slot).unwrap();
        world.restart();
        assert!(!world.core.access(&slot).is_writable());

        // The settings file is the only thing Vault Key has written, and it is not in SaveData.
        assert_eq!(file_names(world.temp.path()), ["SaveData", "VaultKey"]);
        assert_eq!(file_names(&world.temp.path().join("VaultKey")), ["settings.txt"]);
    }

    #[test]
    fn a_test_slot_mark_covers_one_character_and_follows_a_rename_made_here() {
        let mut world = World::new();
        world.put("Save0001.sav", 1);
        let slot = world.slot("Save0001.sav");
        // Slot 1 can be marked like any other slot.
        world.core.mark_test_slot(&slot).unwrap();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));

        // Renaming the character in Vault Key keeps the slot writable, also after a restart.
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Name("Renamed".to_string())).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));
        world.restart();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));
        // So does going back to the backup from before the rename.
        world.core.restore_backup(&backup.id).unwrap();
        assert_eq!(world.core.access(&slot), WriteAccess::Writable(WritableBecause::TestSlot));

        // Another character arrives under the same file name (a main character restored
        // from elsewhere): the mark does not cover it, and nothing can be written over it.
        let mut other = SaveDocument::from_file_bytes(&sample_bytes(1)).unwrap();
        other.set_name("The Real One").unwrap();
        let main = other.to_file_bytes().unwrap();
        fs::write(world.dir.join("Save0001.sav"), &main).unwrap();
        assert_eq!(world.core.access(&slot), WriteAccess::ReadOnly(policy::OTHER_CHARACTER_MESSAGE.to_string()));
        let list = world.core.list_characters().unwrap();
        assert!(!list[0].writable);
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::ReadOnly);
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::ReadOnly);
        assert_eq!(world.read("Save0001.sav"), main);
        world.restart();
        assert!(!world.core.access(&slot).is_writable());

        // Marking again is the user's decision about the new character.
        world.core.mark_test_slot(&slot).unwrap();
        assert!(world.core.access(&slot).is_writable());
    }

    #[test]
    fn a_protected_character_is_never_written() {
        let mut world = World::new();
        let main = world.put("Save0001.sav", 1);
        let slot = world.slot("Save0001.sav");
        world.core.mark_test_slot(&slot).unwrap();
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(20)).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        let saved = world.read("Save0001.sav");

        world.core.protect(&slot).unwrap();
        world.restart();
        assert_eq!(world.core.access(&slot), WriteAccess::ReadOnly(policy::PROTECTED_MESSAGE.to_string()));
        assert!(world.core.list_characters().unwrap()[0].protected);
        assert_eq!(world.core.mark_test_slot(&slot).unwrap_err().kind, ErrorKind::ReadOnly);
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::ReadOnly);
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::ReadOnly);
        assert_eq!(world.read("Save0001.sav"), saved);
        let _ = main;

        // A copy is a new character, and can be edited.
        let copy = world.core.save_as_new(&session, 3).unwrap().slot_id;
        assert!(world.core.access(&copy).is_writable());
    }

    #[test]
    fn a_mark_that_cannot_be_recorded_is_not_kept() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        // The data folder cannot be created: a file has its name.
        fs::write(world.temp.path().join("VaultKey"), b"in the way").unwrap();
        let slot = world.slot("Save0002.sav");
        assert_eq!(world.core.mark_test_slot(&slot).unwrap_err().kind, ErrorKind::Settings);
        assert!(!world.core.access(&slot).is_writable());
    }

    #[test]
    fn write_permission_is_forgotten_when_the_file_is_gone() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        world.put("Save0004.sav", 4);
        let source = world.slot("Save0002.sav");
        let marked = world.slot("Save0004.sav");
        world.core.mark_test_slot(&marked).unwrap();
        let session = world.core.open(&source).unwrap();
        let created = world.core.save_as_new(&session, 3).unwrap().slot_id;
        assert!(world.core.access(&created).is_writable() && world.core.access(&marked).is_writable());

        // Both characters are deleted (in the game, say), and the list is refreshed.
        fs::remove_file(world.dir.join("Save0003.sav")).unwrap();
        fs::remove_file(world.dir.join("Save0004.sav")).unwrap();
        world.core.list_characters().unwrap();
        // The game then makes new characters under the same file names: they are read-only.
        world.put("Save0003.sav", 3);
        world.put("Save0004.sav", 4);
        assert!(!world.core.access(&created).is_writable());
        assert!(!world.core.access(&marked).is_writable());
        world.restart();
        assert!(world.core.list_characters().unwrap().iter().all(|c| !c.writable));
    }

    #[test]
    fn save_as_new_creates_a_writable_character_and_leaves_the_source_alone() {
        let mut world = World::new();
        let original = world.put("Save0001.sav", 1);
        let source = world.slot("Save0001.sav");
        let mut session = world.core.open(&source).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        session.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        session.stage(Edit::BackpackSlots(39)).unwrap();
        session.duplicate_item(weapon_ref()).unwrap();

        let report = world.core.save_as_new(&session, 3).unwrap();
        assert!(report.created_new && report.backup.is_none() && report.warning.is_none());
        assert_eq!(report.slot_id, world.slot("Save0003.sav"));
        assert_eq!(world.read("Save0001.sav"), original, "the source is never written");
        assert_eq!(session.pending_count(), 4, "the session is unchanged");

        let new = SaveDocument::from_file_bytes(&world.read("Save0003.sav")).unwrap();
        assert_eq!((new.save_game_id(), new.level(), new.money(), new.backpack_slots()), (3, 30, 123_456, 39));
        assert_eq!(new.inventory().iter().filter(|e| e.is_weapon).count(), 2);
        assert_eq!(new.name(), "Gaige");

        // It is recorded as created by the app, and that survives a restart.
        assert_eq!(world.core.access(&report.slot_id), WriteAccess::Writable(WritableBecause::CreatedByApp));
        world.restart();
        assert_eq!(world.core.access(&report.slot_id), WriteAccess::Writable(WritableBecause::CreatedByApp));
        assert!(!world.core.access(&source).is_writable());
        assert!(world.core.list_backups().is_empty());
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0003.sav"]);

        // The same slot again, or a bad slot: refused, nothing changes.
        let written = world.read("Save0003.sav");
        assert_eq!(world.core.save_as_new(&session, 3).unwrap_err().kind, ErrorKind::SlotTaken);
        for slot in [0, 1, 10, 16, 4096] {
            assert_eq!(world.core.save_as_new(&session, slot).unwrap_err().kind, ErrorKind::InvalidTarget, "{slot}");
        }
        assert_eq!(world.read("Save0003.sav"), written);
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0003.sav"]);
    }

    #[test]
    fn save_as_new_refuses_an_id_already_in_use() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        // A save whose ID does not match its file name: slot 5's ID is taken by Save0006.sav.
        world.put("Save0006.sav", 5);
        let session = world.core.open(&world.slot("Save0002.sav")).unwrap();
        let error = world.core.save_as_new(&session, 5).unwrap_err();
        assert_eq!(error.kind, ErrorKind::IdCollision);
        assert!(error.message.contains("Save0006.sav already uses save-game ID 5"), "{}", error.message);
        assert!(!world.dir.join("Save0005.sav").exists());
        assert!(!world.core.access(&world.slot("Save0005.sav")).is_writable());
        // While the game runs, or when that cannot be told: refused.
        world.processes.running.store(true, Ordering::SeqCst);
        assert_eq!(world.core.save_as_new(&session, 3).unwrap_err().kind, ErrorKind::GameRunning);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        assert_eq!(world.core.save_as_new(&session, 3).unwrap_err().kind, ErrorKind::GameStatusUnknown);
        assert_eq!(file_names(&world.dir), ["Save0002.sav", "Save0006.sav"]);
    }

    #[test]
    fn save_in_place_backs_up_verifies_and_continues_the_session() {
        let mut world = World::new();
        let original = world.put("Save0004.sav", 4);
        let slot = world.slot("Save0004.sav");
        world.core.mark_test_slot(&slot).unwrap();
        let steps = Arc::new(Mutex::new(Vec::new()));
        let seen = steps.clone();
        world.core.set_progress_listener(Some(Box::new(move |step| seen.lock().unwrap().push(step))));

        let mut session = world.core.open(&slot).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::NothingToSave);
        session.stage(Edit::Level(30)).unwrap();
        session.stage(Edit::Name("Kai".to_string())).unwrap();
        session.stage(Edit::UnlockUvhm).unwrap();
        session.stage(Edit::UnlockAmmo).unwrap();
        session.stage(Edit::OpLevel(2)).unwrap();
        session.stage(Edit::BankSlots(10)).unwrap();
        session.stage(Edit::ImportCodes { codes: vec![crate::testutil::WEAPON_CODE.to_string()], to_bank: true }).unwrap();
        session.duplicate_item(weapon_ref()).unwrap();
        let expected = session.working().clone();

        let report = world.core.save_in_place(&mut session).unwrap();
        assert_eq!(*steps.lock().unwrap(), WriteStep::ALL);
        assert!(!report.created_new && report.warning.is_none());
        let backup = report.backup.unwrap();
        assert_eq!(fs::read(&backup.path).unwrap(), original);
        assert_eq!(backup.reason, "Before saving 8 changes");
        assert_eq!((backup.character_name.as_deref(), backup.level), (Some("Gaige"), Some(5)));
        assert_eq!(backup.created_utc, "2026-11-09 03:15:00 UTC");

        let on_disk = world.read("Save0004.sav");
        assert_eq!(SaveDocument::from_file_bytes(&on_disk).unwrap(), expected);
        // The session carries on from what was saved.
        assert!(!session.has_changes());
        assert_eq!(session.original_bytes(), on_disk);
        assert_eq!(*session.original(), expected);

        // A second save, in the same second: its own backup, of the first save's result.
        session.stage(Edit::Currency(Currency::Eridium, 50)).unwrap();
        let second = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        assert_ne!(second.id, backup.id);
        assert_eq!(second.id, format!("{}-2", backup.id));
        assert_eq!(second.reason, "Before saving 1 change");
        assert_eq!(fs::read(&second.path).unwrap(), on_disk);
        assert_eq!(fs::read(&backup.path).unwrap(), original, "the first backup is untouched");
        let listed: Vec<String> = world.core.list_backups_for(&slot).into_iter().map(|b| b.id).collect();
        assert_eq!(listed, [second.id, backup.id]);
        assert_eq!(file_names(&world.dir), ["Save0004.sav"], "no temp files, and no backups in SaveData");
    }

    #[test]
    fn a_failed_save_leaves_the_file_and_the_session_as_they_were() {
        let mut world = World::new();
        let original = world.put("Save0004.sav", 4);
        let slot = world.slot("Save0004.sav");
        world.core.mark_test_slot(&slot).unwrap();
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();

        world.processes.running.store(true, Ordering::SeqCst);
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::GameRunning);
        world.processes.running.store(false, Ordering::SeqCst);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::GameStatusUnknown);
        world.processes.unreadable.store(false, Ordering::SeqCst);
        assert_eq!(world.read("Save0004.sav"), original);
        assert_eq!(session.pending_count(), 1);
        assert!(world.core.list_backups().is_empty());

        // The game saved the character while the session was open.
        let played = {
            let mut doc = crate::testutil::sample_doc(4);
            doc.set_currency(Currency::Money, 81).unwrap();
            doc.to_file_bytes().unwrap()
        };
        fs::write(world.dir.join("Save0004.sav"), &played).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::ChangedOnDisk);
        assert_eq!(world.read("Save0004.sav"), played);
        assert_eq!(session.pending_count(), 1);
        assert!(world.core.list_backups().is_empty());
        assert_eq!(file_names(&world.dir), ["Save0004.sav"]);

        // Reopened, the same edit saves.
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        world.core.save_in_place(&mut session).unwrap();
        let saved = SaveDocument::from_file_bytes(&world.read("Save0004.sav")).unwrap();
        assert_eq!((saved.level(), saved.money()), (30, 81));
    }

    #[test]
    fn restore_goes_through_the_same_write_path_and_is_itself_backed_up() {
        let mut world = World::new();
        let original = world.put("Save0004.sav", 4);
        let slot = world.slot("Save0004.sav");
        world.core.mark_test_slot(&slot).unwrap();
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        let edited = world.read("Save0004.sav");
        world.now.fetch_add(90, Ordering::SeqCst);

        // Game-checked.
        world.processes.running.store(true, Ordering::SeqCst);
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::GameRunning);
        world.processes.running.store(false, Ordering::SeqCst);
        world.processes.unreadable.store(true, Ordering::SeqCst);
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::GameStatusUnknown);
        world.processes.unreadable.store(false, Ordering::SeqCst);
        assert_eq!(world.read("Save0004.sav"), edited);
        assert_eq!(world.core.list_backups().len(), 1);

        let report = world.core.restore_backup(&backup.id).unwrap();
        assert_eq!(world.read("Save0004.sav"), original, "the file is the backup, byte for byte");
        // What was there before the restore was backed up first, so the restore can be undone.
        let before_restore = report.backup.unwrap();
        assert_eq!(fs::read(&before_restore.path).unwrap(), edited);
        assert_eq!(before_restore.reason, "Before restoring an earlier backup");
        assert_eq!(before_restore.created_utc, "2026-11-09 03:16:30 UTC");
        let listed: Vec<String> = world.core.list_backups().into_iter().map(|b| b.id).collect();
        assert_eq!(listed, [before_restore.id.clone(), backup.id.clone()]);
        assert_eq!(fs::read(&backup.path).unwrap(), original, "restoring does not consume the backup");

        // The open session is now out of date, and says so instead of overwriting.
        session.stage(Edit::Level(40)).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::ChangedOnDisk);
        assert_eq!(world.read("Save0004.sav"), original);

        // Undo the restore.
        world.core.restore_backup(&before_restore.id).unwrap();
        assert_eq!(world.read("Save0004.sav"), edited);
        assert_eq!(file_names(&world.dir), ["Save0004.sav"]);
    }

    #[test]
    fn restore_refuses_missing_damaged_and_non_save_backups() {
        let mut world = World::new();
        world.put("Save0004.sav", 4);
        let slot = world.slot("Save0004.sav");
        world.core.mark_test_slot(&slot).unwrap();
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();

        assert_eq!(world.core.restore_backup("nonsense").unwrap_err().kind, ErrorKind::NoSuchBackup);
        assert_eq!(world.core.restore_backup(&format!("{STEAM_ID}/Save0004.sav/20200101-000000Z")).unwrap_err().kind, ErrorKind::NoSuchBackup);
        assert_eq!(world.core.restore_backup("../../Save0004.sav").unwrap_err().kind, ErrorKind::NoSuchBackup);

        // The backup file was altered after it was taken.
        let good = fs::read(&backup.path).unwrap();
        fs::write(&backup.path, sample_bytes(9)).unwrap();
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::BadBackup);
        fs::write(&backup.path, &good).unwrap();

        // The current file is not a save (damaged). Restoring over it backs the damaged
        // bytes up; that backup can be listed but is refused as a restore source.
        fs::write(world.dir.join("Save0004.sav"), b"damaged by something").unwrap();
        let report = world.core.restore_backup(&backup.id).unwrap();
        assert_eq!(world.read("Save0004.sav"), good);
        let damaged = report.backup.unwrap();
        assert_eq!((damaged.character_name.as_deref(), damaged.level), (None, None));
        let error = world.core.restore_backup(&damaged.id).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadBackup);
        assert!(error.message.contains("not a readable save"), "{}", error.message);
        assert_eq!(world.read("Save0004.sav"), good);
    }

    #[test]
    fn restore_into_a_slot_whose_file_is_gone_follows_the_rules_for_new_files() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        let session = world.core.open(&world.slot("Save0002.sav")).unwrap();
        let created = world.core.save_as_new(&session, 3).unwrap().slot_id;
        let mut session = world.core.open(&created).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        let backed_up = fs::read(&backup.path).unwrap();

        // The character is deleted; the list is refreshed, which forgets the permission.
        fs::remove_file(world.dir.join("Save0003.sav")).unwrap();
        world.core.list_characters().unwrap();
        assert!(!world.core.access(&created).is_writable());

        // Another save now uses ID 3: the backup must not come back beside it.
        world.put("Save0008.sav", 3);
        assert_eq!(world.core.restore_backup(&backup.id).unwrap_err().kind, ErrorKind::IdCollision);
        assert!(!world.dir.join("Save0003.sav").exists());
        fs::remove_file(world.dir.join("Save0008.sav")).unwrap();

        let report = world.core.restore_backup(&backup.id).unwrap();
        assert!(report.created_new && report.backup.is_none());
        assert_eq!(world.read("Save0003.sav"), backed_up);
        assert!(world.core.access(&created).is_writable(), "Vault Key created this file");
    }

    /// A folder outside the save folder, for files opened and exported by hand.
    fn outside(world: &World) -> PathBuf {
        let dir = world.temp.path().join("Outside");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_from_elsewhere_opens_as_an_external_character_and_is_never_written() {
        let mut world = World::new();
        let dir = outside(&world);
        let source = dir.join("Save0002.sav");
        let bytes = sample_bytes(2);
        fs::write(&source, &bytes).unwrap();

        let mut session = world.core.open_file(&source).unwrap();
        assert_eq!(session.source(), &crate::session::SessionSource::External(source.clone()));
        assert_eq!((session.slot_id(), session.file_name().as_str()), (None, "Save0002.sav"));
        session.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        session.duplicate_item(weapon_ref()).unwrap();
        // It has no place in the save folder to be saved into.
        let error = world.core.save_in_place(&mut session).unwrap_err();
        assert_eq!((error.kind, error.message.as_str()), (ErrorKind::ReadOnly, EXTERNAL_MESSAGE));
        assert_eq!(fs::read(&source).unwrap(), bytes, "the source is byte for byte as it was");
        assert_eq!(file_names(&dir), ["Save0002.sav"]);
        assert_eq!(file_names(&world.dir), Vec::<String>::new(), "opening it put nothing in the save folder");

        // Exporting over the file it came from replaces it, and what was there is kept.
        let report = world.core.export(&session, &source).unwrap();
        let kept = PathBuf::from(report.replaced_backup.expect("the old file is kept"));
        assert_eq!(fs::read(&kept).unwrap(), bytes, "the kept copy is the file as it was");
        let written = SaveDocument::from_file_bytes(&fs::read(&source).unwrap()).unwrap();
        assert_eq!(&written, session.working());
        assert_eq!((written.money(), written.save_game_id()), (123_456, 2));
        assert_eq!(file_names(&dir), ["Save0002.sav"], "no temporary file is left behind");
        // The changes are still pending, and it opens again with them in it.
        assert!(session.pending_count() > 0);
        assert_eq!(world.core.open_file(&source).unwrap().working(), session.working());

        // Not a save, too big, not there, a folder, a relative path.
        fs::write(dir.join("notes.sav"), b"hello").unwrap();
        assert_eq!(world.core.open_file(&dir.join("notes.sav")).unwrap_err().kind, ErrorKind::Unreadable);
        let big = fs::File::create(dir.join("big.sav")).unwrap();
        big.set_len(MAX_SAVE_FILE_BYTES + 1).unwrap();
        drop(big);
        assert!(world.core.open_file(&dir.join("big.sav")).unwrap_err().message.contains("bigger than any save"));
        assert_eq!(world.core.open_file(&dir.join("gone.sav")).unwrap_err().kind, ErrorKind::Unreadable);
        assert_eq!(world.core.open_file(&dir).unwrap_err().kind, ErrorKind::Unreadable);
        assert_eq!(world.core.open_file(Path::new("Save0002.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
    }

    #[test]
    fn lock_and_unlock_switch_a_character_between_never_written_and_editable() {
        let mut world = World::new();
        world.put("Save0001.sav", 1);
        world.put("Save0002.sav", 2);
        let (main, other) = (world.slot("Save0001.sav"), world.slot("Save0002.sav"));
        // Out of the box a character is locked; unlocking makes it editable, slot 1 too.
        assert!(!world.core.access(&main).is_writable());
        world.core.unlock(&main).unwrap();
        assert_eq!(world.core.access(&main), WriteAccess::Writable(WritableBecause::TestSlot));
        assert!(!world.core.access(&other).is_writable(), "unlocking one character leaves the others");
        // Locking protects it, and the lock outlives a restart.
        world.core.lock(&main).unwrap();
        assert_eq!(world.core.access(&main), WriteAccess::ReadOnly(policy::PROTECTED_MESSAGE.to_string()));
        let again = Settings::load(&world.core.settings_path()).unwrap();
        assert_eq!((again.protected().len(), again.test_slots().len()), (1, 0));
        let mut session = world.core.open(&main).unwrap();
        session.stage(Edit::Level(9)).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().message, policy::PROTECTED_MESSAGE);
        // Unlocking a protected character takes the protection off and it saves.
        world.core.unlock(&main).unwrap();
        assert!(Settings::load(&world.core.settings_path()).unwrap().protected().is_empty());
        world.core.save_in_place(&mut session).unwrap();
        // What cannot be read or is not there cannot be unlocked.
        assert_eq!(world.core.unlock(&world.slot("Save0005.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
    }

    #[test]
    fn a_file_inside_the_save_folder_opens_as_its_slot() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        let session = world.core.open_file(&world.dir.join("Save0002.sav")).unwrap();
        assert_eq!(session.slot_id(), Some(&world.slot("Save0002.sav")));
        // Spelled with other capitals and a detour, it is still the slot.
        let odd = world.temp.path().join("SAVEDATA").join(STEAM_ID).join("..").join(STEAM_ID).join("save0002.sav");
        if odd.exists() {
            assert!(world.core.open_file(&odd).unwrap().slot_id().is_some());
        }
        // And it follows the slot's rules: read-only until marked.
        let mut session = session;
        session.stage(Edit::Level(9)).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().message, policy::READ_ONLY_MESSAGE);
    }

    #[test]
    fn export_writes_the_edited_character_verified_and_keeps_its_id() {
        let mut world = World::new();
        world.put("Save0004.sav", 4);
        let dir = outside(&world);
        let slot = world.slot("Save0004.sav");
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        session.duplicate_item(weapon_ref()).unwrap();
        let before = world.read("Save0004.sav");

        let target = dir.join("Save0004.sav");
        let report = world.core.export(&session, &target).unwrap();
        assert_eq!((report.file_name.as_str(), report.save_game_id, report.level), ("Save0004.sav", 4, 30));
        assert_eq!((report.name.as_str(), report.replaced_backup.as_ref()), ("Gaige", None));
        let exported = SaveDocument::from_file_bytes(&fs::read(&target).unwrap()).unwrap();
        assert_eq!(&exported, session.working());
        assert_eq!((exported.money(), exported.save_game_id()), (123_456, 4));
        assert_eq!(exported.inventory().iter().filter(|e| e.is_weapon).count(), 2);
        // The session keeps its pending changes, its file is untouched, and no temp file is left.
        assert_eq!(session.pending_count(), 3);
        assert_eq!(world.read("Save0004.sav"), before);
        assert_eq!(file_names(&dir), ["Save0004.sav"]);

        // Exported again over the same file: what was there is kept first.
        session.stage(Edit::Currency(Currency::Eridium, 77)).unwrap();
        let old = fs::read(&target).unwrap();
        let report = world.core.export(&session, &target).unwrap();
        let kept = PathBuf::from(report.replaced_backup.unwrap());
        assert!(kept.starts_with(world.core.backups_dir().join("exports")), "{}", kept.display());
        assert_eq!(fs::read(&kept).unwrap(), old);
        assert_eq!(SaveDocument::from_file_bytes(&fs::read(&target).unwrap()).unwrap().eridium(), 77);
        // A third time in the same second: another backup beside the first, not over it.
        let report = world.core.export(&session, &target).unwrap();
        assert_ne!(PathBuf::from(report.replaced_backup.unwrap()), kept);
        assert_eq!(fs::read(&kept).unwrap(), old);
        assert_eq!(file_names(&dir), ["Save0004.sav"]);
        // Backups of exports are not offered as character backups to restore.
        assert!(world.core.list_backups().is_empty());
    }

    #[test]
    fn export_refuses_save_folders_the_game_and_vault_keys_own_folder() {
        let mut world = World::new();
        world.put("Save0004.sav", 4);
        let session = world.core.open(&world.slot("Save0004.sav")).unwrap();
        let refused = |core: &mut Core, target: PathBuf| {
            let error = core.export(&session, &target).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidTarget, "{}", target.display());
            assert!(!target.exists() || target.ends_with("Save0004.sav"), "{}", target.display());
            error.message
        };
        assert!(refused(&mut world.core, world.dir.join("Save0009.sav")).contains("save folder"));
        assert!(refused(&mut world.core, world.dir.join("Save0004.sav")).contains("save folder"));
        assert!(refused(&mut world.core, world.temp.path().join("SaveData").join("x.sav")).contains("save folder"));
        // A Borderlands 2 save folder Vault Key was not told about.
        let other = world.temp.path().join("Docs").join("My Games").join("Borderlands 2").join("WillowGame").join("SaveData").join("123");
        fs::create_dir_all(&other).unwrap();
        assert!(refused(&mut world.core, other.join("Save0002.sav")).contains("save folder"));
        // The game install (any Steam library).
        let game = world.temp.path().join("Steam").join("steamapps").join("common");
        fs::create_dir_all(&game).unwrap();
        assert!(refused(&mut world.core, game.join("Save0004.sav")).contains("Steam library"));
        // Vault Key's own folder.
        fs::create_dir_all(world.temp.path().join("VaultKey").join("backups")).unwrap();
        assert!(refused(&mut world.core, world.temp.path().join("VaultKey").join("backups").join("x.sav")).contains("own folder"));
        // Relative, no name, a missing folder, a folder in the way.
        assert_eq!(world.core.export(&session, Path::new("x.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
        assert!(world.core.export(&session, &world.temp.path().join("nowhere").join("x.sav")).unwrap_err().message.contains("does not exist"));
        let dir = outside(&world);
        fs::create_dir_all(dir.join("a.sav")).unwrap();
        assert_eq!(world.core.export(&session, &dir.join("a.sav")).unwrap_err().kind, ErrorKind::InvalidTarget);
        assert_eq!(file_names(&world.dir), ["Save0004.sav"]);
    }

    #[test]
    fn a_protected_character_can_be_exported_and_stays_as_it_was() {
        let mut world = World::new();
        let main = world.put("Save0001.sav", 1);
        let slot = world.slot("Save0001.sav");
        world.core.protect(&slot).unwrap();
        let mut session = world.core.open(&slot).unwrap();
        session.stage(Edit::Currency(Currency::Money, 999)).unwrap();
        let target = outside(&world).join("Save0001.sav");
        let report = world.core.export(&session, &target).unwrap();
        assert_eq!(report.save_game_id, 1);
        assert_eq!(SaveDocument::from_file_bytes(&fs::read(&target).unwrap()).unwrap().money(), 999);
        assert_eq!(world.read("Save0001.sav"), main);
        assert_eq!(world.core.access(&slot), WriteAccess::ReadOnly(policy::PROTECTED_MESSAGE.to_string()));
    }

    #[test]
    fn an_external_character_is_added_to_my_characters_with_a_fresh_id() {
        let mut world = World::new();
        world.put("Save0002.sav", 2);
        let dir = outside(&world);
        // A friend's character, in their slot 2 with ID 2: the same as one of ours.
        let source = dir.join("Save0002.sav");
        let bytes = sample_bytes(2);
        fs::write(&source, &bytes).unwrap();
        let mut session = world.core.open_file(&source).unwrap();
        session.stage(Edit::Currency(Currency::Money, 4242)).unwrap();

        // Slot 2 is taken; another Steam ID folder does not exist.
        assert_eq!(world.core.save_as_new(&session, 2).unwrap_err().kind, ErrorKind::SlotTaken);
        assert_eq!(world.core.save_as_new_in(&session, "999", 3).unwrap_err().kind, ErrorKind::InvalidTarget);
        let report = world.core.save_as_new(&session, 3).unwrap();
        assert_eq!(report.slot_id, world.slot("Save0003.sav"));
        let added = SaveDocument::from_file_bytes(&world.read("Save0003.sav")).unwrap();
        assert_eq!((added.save_game_id(), added.money()), (3, 4242));
        assert!(world.core.access(&report.slot_id).is_writable());
        assert_eq!(fs::read(&source).unwrap(), bytes, "the friend's file is untouched");

        // With two Steam-ID folders, the user must say which.
        fs::create_dir_all(world.temp.path().join("SaveData").join("76561190000000001")).unwrap();
        assert_eq!(world.core.save_as_new(&session, 4).unwrap_err().kind, ErrorKind::InvalidTarget);
        let report = world.core.save_as_new_in(&session, "76561190000000001", 2).unwrap();
        assert_eq!(report.slot_id.steam_id(), "76561190000000001");
    }

    #[test]
    fn the_core_can_cross_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<Core>();
        assert_send::<CoreConfig>();
    }

    /// The whole flow on a synthetic character, so it runs even without fixture saves.
    #[test]
    fn end_to_end_on_a_synthetic_character() {
        let mut world = World::new();
        let source_bytes = world.put("Save0001.sav", 1);
        let source = world.core.list_characters().unwrap().remove(0);
        assert!(source.is_readable() && !source.writable);

        let mut session = world.core.open(&source.slot_id).unwrap();
        session.stage(Edit::Level(30)).unwrap();
        session.stage(Edit::Currency(Currency::Money, 123_456)).unwrap();
        session.stage(Edit::BackpackSlots(39)).unwrap();
        session.duplicate_item(weapon_ref()).unwrap();
        assert_eq!(world.core.save_in_place(&mut session).unwrap_err().kind, ErrorKind::ReadOnly);

        let new = world.core.save_as_new(&session, 3).unwrap().slot_id;
        let mut session = world.core.open(&new).unwrap();
        let snapshot = session.snapshot();
        assert_eq!((snapshot.level, snapshot.money, snapshot.backpack_slots, snapshot.save_game_id), (30, 123_456, 39, 3));
        assert_eq!(session.items().iter().filter(|i| i.is_weapon).count(), 2);

        let pre_edit = world.read("Save0003.sav");
        session.stage(Edit::Currency(Currency::Eridium, 99)).unwrap();
        let backup = world.core.save_in_place(&mut session).unwrap().backup.unwrap();
        assert_ne!(world.read("Save0003.sav"), pre_edit);
        assert_eq!(world.core.open(&new).unwrap().snapshot().eridium, 99);

        world.core.restore_backup(&backup.id).unwrap();
        assert_eq!(world.read("Save0003.sav"), pre_edit);
        assert_eq!(world.read("Save0001.sav"), source_bytes);
        assert_eq!(file_names(&world.dir), ["Save0001.sav", "Save0003.sav"]);
    }
}
