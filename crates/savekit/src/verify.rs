//! The post-write check: decode what was written and compare it with what was meant.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::document::SaveDocument;
use crate::error::{Result, SaveKitError};
use crate::items::inventory::{EntryRef, InventoryEntry};

/// A character number that [`Expectation::CharacterValue`] can check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharacterKey {
    Level,
    Experience,
    Money,
    Eridium,
    Seraph,
    Torgue,
    BackpackSlots,
    BankSlots,
    GunSlots,
    OpLevel,
    SaveGameId,
}

impl CharacterKey {
    pub const ALL: [CharacterKey; 11] = [
        CharacterKey::Level,
        CharacterKey::Experience,
        CharacterKey::Money,
        CharacterKey::Eridium,
        CharacterKey::Seraph,
        CharacterKey::Torgue,
        CharacterKey::BackpackSlots,
        CharacterKey::BankSlots,
        CharacterKey::GunSlots,
        CharacterKey::OpLevel,
        CharacterKey::SaveGameId,
    ];

    /// The key's name, as used in messages and by [`FromStr`].
    pub fn name(self) -> &'static str {
        match self {
            CharacterKey::Level => "level",
            CharacterKey::Experience => "experience",
            CharacterKey::Money => "money",
            CharacterKey::Eridium => "eridium",
            CharacterKey::Seraph => "seraph",
            CharacterKey::Torgue => "torgue",
            CharacterKey::BackpackSlots => "backpackSlots",
            CharacterKey::BankSlots => "bankSlots",
            CharacterKey::GunSlots => "gunSlots",
            CharacterKey::OpLevel => "opLevel",
            CharacterKey::SaveGameId => "saveGameID",
        }
    }

    /// Reads this value from a document.
    pub fn read(self, doc: &SaveDocument) -> i64 {
        match self {
            CharacterKey::Level => doc.level(),
            CharacterKey::Experience => doc.experience(),
            CharacterKey::Money => doc.money(),
            CharacterKey::Eridium => doc.eridium(),
            CharacterKey::Seraph => doc.seraph(),
            CharacterKey::Torgue => doc.torgue(),
            CharacterKey::BackpackSlots => doc.backpack_slots(),
            CharacterKey::BankSlots => doc.bank_slots(),
            CharacterKey::GunSlots => doc.gun_slots(),
            CharacterKey::OpLevel => doc.op_level(),
            CharacterKey::SaveGameId => doc.save_game_id(),
        }
    }
}

impl fmt::Display for CharacterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for CharacterKey {
    type Err = SaveKitError;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|key| key.name() == s).ok_or_else(|| fail(format!("unknown key {s}")))
    }
}

/// Something a written save file must satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// The written inventory holds exactly `count` more entries with the same item bytes
    /// as `source` (looked up in the original document) than the original did. With any
    /// `Duplicated` expectation, the inventory must also have grown by exactly the total of
    /// their counts.
    Duplicated { source: EntryRef, count: usize },
    /// The original's item at `from` is at `to` in the written file (it was moved between
    /// the backpack and the bank, or moved up when an item before it was). The check that
    /// every original item is still there and still decodes looks for it at `to`.
    Relocated { from: EntryRef, to: EntryRef },
    CharacterValue(CharacterKey, i64),
    Name(String),
    /// The whole message equals the original's.
    Unchanged,
}

fn fail(reason: impl Into<String>) -> SaveKitError {
    SaveKitError::invalid("verify", reason)
}

/// Entries whose item bytes equal `item_bytes`.
fn copies(inventory: &[InventoryEntry], item_bytes: &[u8]) -> usize {
    inventory.iter().filter(|e| e.blob.as_ref().is_some_and(|b| b.item_bytes == item_bytes)).count()
}

/// Decodes `written_file` from its bytes (the whole codec, and every item checksum) and
/// checks each expectation. Whatever the expectations, every item that decoded in `original`
/// must still be at the same place (or where a [`Expectation::Relocated`] says it went) and
/// still decode.
///
/// Returns the first problem: a decoding error as it is, anything else as
/// [`SaveKitError::InvalidValue`] with `field == "verify"`.
pub fn verify(original: &SaveDocument, written_file: &[u8], expectations: &[Expectation]) -> Result<()> {
    let written = SaveDocument::from_file_bytes(written_file)?;
    let original_inventory = original.inventory();
    let written_inventory = written.inventory();

    let written_by_ref: HashMap<EntryRef, &InventoryEntry> =
        written_inventory.iter().map(|e| (e.entry_ref, e)).collect();
    let relocated: HashMap<EntryRef, EntryRef> = expectations
        .iter()
        .filter_map(|e| match e {
            Expectation::Relocated { from, to } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    for entry in original_inventory.iter().filter(|e| e.blob.is_some()) {
        let now = relocated.get(&entry.entry_ref).copied().unwrap_or(entry.entry_ref);
        match written_by_ref.get(&now) {
            None => return Err(fail(format!("item {} is missing from the written file", entry.entry_ref))),
            Some(w) if w.blob.is_none() => {
                return Err(fail(format!("item {} failed its checksum in the written file", entry.entry_ref)))
            }
            Some(_) => {}
        }
    }

    let duplicate_counts: Vec<usize> = expectations
        .iter()
        .filter_map(|e| match e {
            Expectation::Duplicated { count, .. } => Some(*count),
            _ => None,
        })
        .collect();
    if !duplicate_counts.is_empty() {
        let added = duplicate_counts.iter().fold(0usize, |total, &count| total.saturating_add(count));
        let expected = original_inventory.len().saturating_add(added);
        if written_inventory.len() != expected {
            return Err(fail(format!(
                "inventory size: expected {expected} entries, found {}",
                written_inventory.len()
            )));
        }
    }

    for expectation in expectations {
        match expectation {
            Expectation::Unchanged => {
                if written.message != original.message {
                    return Err(fail("expected the save to be unchanged, but its contents differ"));
                }
            }
            Expectation::Name(expected) => {
                let actual = written.name();
                if actual != *expected {
                    return Err(fail(format!("name: expected \"{expected}\", found \"{actual}\"")));
                }
            }
            Expectation::CharacterValue(key, expected) => {
                let actual = key.read(&written);
                if actual != *expected {
                    return Err(fail(format!("{key}: expected {expected}, found {actual}")));
                }
            }
            Expectation::Relocated { .. } => {}
            Expectation::Duplicated { source, count } => {
                let source_blob = original_inventory
                    .iter()
                    .find(|e| e.entry_ref == *source)
                    .and_then(|e| e.blob.as_ref())
                    .ok_or_else(|| {
                        fail(format!("duplicate source {source} not found (or undecodable) in the original"))
                    })?;
                let before = copies(&original_inventory, &source_blob.item_bytes);
                let after = copies(&written_inventory, &source_blob.item_bytes);
                if after.checked_sub(before) != Some(*count) {
                    return Err(fail(format!(
                        "duplicates: expected {count} new copies, found {}",
                        after as i64 - before as i64
                    )));
                }
            }
        }
    }
    Ok(())
}
