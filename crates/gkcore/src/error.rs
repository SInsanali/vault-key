//! The one error type of the application core.

use std::fmt;

use crate::guards::GkError;

/// What went wrong, as a category a front end can switch on. The wording for the user is in
/// [`CoreError::message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The folders Vault Key was told to use are not acceptable (for example, backups inside
    /// the save folder).
    Config,
    /// The SaveData folder, or something in it, cannot be found or listed.
    SaveFolder,
    /// A file is not a save Vault Key can read.
    Unreadable,
    /// The settings file cannot be read or written.
    Settings,

    /// Policy: this file is read-only to Vault Key.
    ReadOnly,
    /// The place a save was to be written is not acceptable (bad slot, outside the save folder).
    InvalidTarget,
    /// A new character was to be created in a slot that already holds a file.
    SlotTaken,
    /// Two saves would share a save-game ID.
    IdCollision,
    /// The game is running.
    GameRunning,
    /// The process list could not be read, so the game may be running.
    GameStatusUnknown,
    /// The file on disk is no longer what the session was opened from.
    ChangedOnDisk,
    /// The backup could not be made.
    BackupFailed,
    /// The temporary file could not be written.
    WriteFailed,
    /// The written file did not pass verification.
    VerificationFailed,
    /// The temporary file could not be moved into place.
    ReplaceFailed,
    /// The file read back after the move was not the verified one.
    ConfirmFailed,

    /// A value is out of range or not acceptable.
    InvalidValue,
    BackpackFull,
    BankFull,
    NotDuplicable,
    /// The edit cannot be combined with another pending change.
    Conflict,
    /// There is no pending change with that id.
    NoSuchChange,
    /// Save was asked for with nothing staged.
    NothingToSave,

    /// There is no backup with that id.
    NoSuchBackup,
    /// The backup exists but is damaged or is not a save.
    BadBackup,
}

impl ErrorKind {
    /// A stable, lower-case name for passing across a message boundary.
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::Config => "config",
            ErrorKind::SaveFolder => "save-folder",
            ErrorKind::Unreadable => "unreadable",
            ErrorKind::Settings => "settings",
            ErrorKind::ReadOnly => "read-only",
            ErrorKind::InvalidTarget => "invalid-target",
            ErrorKind::SlotTaken => "slot-taken",
            ErrorKind::IdCollision => "id-collision",
            ErrorKind::GameRunning => "game-running",
            ErrorKind::GameStatusUnknown => "game-status-unknown",
            ErrorKind::ChangedOnDisk => "changed-on-disk",
            ErrorKind::BackupFailed => "backup-failed",
            ErrorKind::WriteFailed => "write-failed",
            ErrorKind::VerificationFailed => "verification-failed",
            ErrorKind::ReplaceFailed => "replace-failed",
            ErrorKind::ConfirmFailed => "confirm-failed",
            ErrorKind::InvalidValue => "invalid-value",
            ErrorKind::BackpackFull => "backpack-full",
            ErrorKind::BankFull => "bank-full",
            ErrorKind::NotDuplicable => "not-duplicable",
            ErrorKind::Conflict => "conflict",
            ErrorKind::NoSuchChange => "no-such-change",
            ErrorKind::NothingToSave => "nothing-to-save",
            ErrorKind::NoSuchBackup => "no-such-backup",
            ErrorKind::BadBackup => "bad-backup",
        }
    }
}

/// A refusal or a failure, with a message written for the person using the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub kind: ErrorKind,
    pub message: String,
    /// The pending change that failed, when the error came from replaying staged edits.
    pub change_id: Option<u64>,
}

impl CoreError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> CoreError {
        CoreError { kind, message: message.into(), change_id: None }
    }

    pub(crate) fn for_change(mut self, change_id: u64) -> CoreError {
        self.change_id = Some(change_id);
        self
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}

pub type Result<T> = std::result::Result<T, CoreError>;

pub(crate) fn fail<T>(kind: ErrorKind, message: impl Into<String>) -> Result<T> {
    Err(CoreError::new(kind, message))
}

/// A guard refusal with the leading "refused: " removed and a capital first letter.
pub(crate) fn from_guard(kind: ErrorKind, error: GkError) -> CoreError {
    let text = error.0.strip_prefix("refused: ").unwrap_or(&error.0);
    let mut chars = text.chars();
    let message = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    CoreError::new(kind, message)
}
