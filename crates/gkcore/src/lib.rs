//! Vault Key's application core: everything between the save engine ([`savekit`]) and a
//! user interface.
//!
//! A front end creates one [`Core`] and calls into it:
//!
//! ```no_run
//! use gkcore::{Core, CoreConfig, Edit};
//!
//! # fn main() -> Result<(), gkcore::CoreError> {
//! let mut core = Core::new(CoreConfig::live()?)?;
//! let characters = core.list_characters()?;
//! let mut session = core.open(&characters[0].slot_id)?;
//! session.stage(Edit::Level(30))?;
//! for line in session.change_lines() {
//!     println!("{}", line.text); // "Level 1 → 30"
//! }
//! // An existing character is read-only unless Vault Key created it or its slot was
//! // marked as a test slot, so the usual first step is a copy in a free slot:
//! let copy = core.save_as_new(&session, 3)?;
//! # let _ = copy;
//! # Ok(())
//! # }
//! ```
//!
//! The rules that protect real characters (design section 6) live here, not in the front
//! end:
//!
//! - [`policy`]: an existing save file may be changed only if this app created it or the
//!   user marked its slot as a test slot (a mark that covers only the character it was made
//!   for). Nothing is writable by default, and there is no switch that allows everything.
//! - [`writer`]: every write to the SaveData folder goes through one function, which checks
//!   the policy, refuses while the game runs, refuses if the file changed since it was
//!   opened, backs up, writes a temp file, verifies it from disk, swaps it in, and confirms.
//! - [`backup`]: backups live in Vault Key's own folder, never in SaveData, and never
//!   overwrite one another.
//! - [`session`] and [`changes`]: edits are staged, and nothing touches the disk until a save.
//!
//! Nothing here panics on file data or on a failing file operation. Errors are
//! [`CoreError`]: a kind to switch on and a message written for the person using the app.
//! Only the standard library, [`savekit`] and [`gamedata`] are used.

#![forbid(unsafe_code)]

pub mod app;
pub mod backup;
pub mod catalog;
pub mod changes;
pub mod error;
pub mod format;
pub mod guards;
pub mod library;
pub mod policy;
pub mod session;
pub mod settings;
pub mod slot;
pub mod time;
pub mod writer;

#[cfg(test)]
pub(crate) mod testutil;

pub use app::{Core, CoreConfig, ExportReport, SaveReport, APP_FOLDER_NAME, DATA_DIR_ENV, OLD_APP_FOLDER_NAME, MAX_SAVE_FILE_BYTES};
pub use backup::{BackupInfo, BackupStore};
pub use catalog::{CardLine, Catalog, ItemCard, ItemDisplay, StatLine};
pub use changes::{
    ammo_capacity, ChangeLine, Edit, PendingChange, AMMO_LABELS, AMMO_NAMES, BACKPACK_FULL_MESSAGE, BANK_FULL_MESSAGE,
    ITEM_GONE_MESSAGE, MAX_GEAR_LEVEL,
};
pub use error::{CoreError, ErrorKind, Result};
pub use library::{CharacterClass, CharacterEntry};
pub use policy::{WritableBecause, WriteAccess};
pub use session::{CharacterSession, CharacterSnapshot, ItemRow, SessionSource, SharedClassifier, StageOutcome};
pub use settings::Settings;
pub use slot::SlotId;
pub use time::{fixed_clock, system_clock, Clock};
pub use writer::{live_process_lister, GameStatus, ProcessLister, WriteStep};

// The engine types that appear in this crate's API, so a front end needs only this crate.
pub use savekit::{
    Currency, EntryRef, FallbackGearClassifier, GearClass, GearClassifier, ItemFields, ItemLocation, SaveDocument,
};
