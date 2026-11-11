//! SaveKit: the Borderlands 2 `.sav` engine. Format reference: `docs/format/bl2-save-format.md`.
//!
//! This crate is a pure library: it turns the bytes of a save file into a [`SaveDocument`]
//! and back, and never touches the file system. Reading, backing up and replacing files is
//! the caller's job.
//!
//! ```
//! use savekit::{verify, CharacterKey, Expectation, SaveDocument};
//!
//! # fn main() -> Result<(), savekit::SaveKitError> {
//! # let file_bytes = SaveDocument::default().to_file_bytes()?;
//! let original = SaveDocument::from_file_bytes(&file_bytes)?;
//! let mut doc = original.clone();
//! doc.set_level(30)?;
//! let written = doc.to_file_bytes()?;
//! verify(&original, &written, &[Expectation::CharacterValue(CharacterKey::Level, 30)])?;
//! # Ok(())
//! # }
//! ```
//!
//! Layers, outermost first: [`codec`] (SHA-1, LZO1X, the WSG header), [`huffman`],
//! [`proto`] (an order-preserving protobuf message), then [`SaveDocument`] with character
//! edits and the inventory ([`ItemBlob`], [`ItemFields`], [`InventoryEntry`]).
//!
//! Nothing here panics on file data: a damaged save gives a [`SaveKitError`].

#![forbid(unsafe_code)]

pub mod checksums;
pub mod codec;
pub mod field;
pub mod huffman;
pub mod proto;
pub mod tables;

mod character;
mod document;
mod error;
mod items;
mod verify;

pub use character::{
    Currency, AMMO_SDU_COUNT, MAX_BACKPACK_SLOTS, MAX_BANK_SLOTS, MAX_CURRENCY, MAX_LEVEL, MIN_BACKPACK_SLOTS, MIN_BANK_SLOTS,
    MIN_LEVEL,
};
pub use document::SaveDocument;
pub use error::{Result, SaveKitError};
pub use items::blob::ItemBlob;
pub use items::fields::{ItemFields, PART_COUNT};
pub use items::inventory::{
    EntryRef, FallbackGearClassifier, GearClass, GearClassifier, InventoryEntry, ItemLocation, MAX_ITEM_LEVEL,
    MAX_OP_LEVEL,
};
pub use items::keys::{EntropyKeys, KeySource};
pub use proto::{ProtoField, ProtoMessage, ProtoValue};
pub use verify::{verify, CharacterKey, Expectation};
