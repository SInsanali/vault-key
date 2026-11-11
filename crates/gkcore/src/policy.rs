//! The write policy (design section 6): which existing files Vault Key may change.
//!
//! A save file may be edited in place only if this app created it, or the user marked its
//! slot as a test slot. Every other existing file is read-only. There is no setting that
//! allows everything. The single write path asks this module itself; callers cannot skip it.
//!
//! A test-slot mark is the user's decision about one character, not about a file name. It is
//! recorded together with a mark of the character that was in the slot ([`character_mark`]),
//! and applies only while that character is still there. So when another character later
//! arrives under the same file name (the user's main character restored from a backup, say),
//! it is read-only until the user decides again. Slot 1 follows the same rules as any other
//! slot, except that Vault Key never creates a file there.

use savekit::SaveDocument;

use crate::guards;
use crate::settings::Settings;
use crate::slot::SlotId;

/// Why a file may be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritableBecause {
    /// Vault Key created the file ("Save as a new character").
    CreatedByApp,
    /// The user marked the slot as a test slot.
    TestSlot,
}

/// Whether Vault Key may change an existing save file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteAccess {
    Writable(WritableBecause),
    /// Read-only, with the reason in words for the user.
    ReadOnly(String),
}

impl WriteAccess {
    pub fn is_writable(&self) -> bool {
        matches!(self, WriteAccess::Writable(_))
    }

    /// One sentence for the user.
    pub fn explanation(&self) -> String {
        match self {
            WriteAccess::Writable(WritableBecause::CreatedByApp) => {
                "Vault Key created this character, so it can be edited in place.".to_string()
            }
            WriteAccess::Writable(WritableBecause::TestSlot) => {
                "This slot is marked as a test slot, so it can be edited in place.".to_string()
            }
            WriteAccess::ReadOnly(why) => why.clone(),
        }
    }
}

pub(crate) const OTHER_CHARACTER_MESSAGE: &str =
    "This slot was marked as a test slot for a different character, so the mark does not apply to this one. Use \"Save to new slot\", or mark the slot again in Settings if this character is a test character too.";
pub(crate) const PROTECTED_MESSAGE: &str =
    "This character is locked: Vault Key never changes its file, whatever else is set. Unlock it to edit it in place, or use \"Save to new slot\" to work on a copy.";
pub(crate) const UNREADABLE_MESSAGE: &str =
    "This file cannot be read, so Vault Key will not edit it. A backup of the test character can still be restored over it.";
pub(crate) const NOT_A_SLOT_MESSAGE: &str =
    "This file does not have a standard slot name (like Save0002.sav), so Vault Key will not write it.";
pub(crate) const READ_ONLY_MESSAGE: &str =
    "This character is locked (read-only): Vault Key will not change its file. Unlock it to save changes into it, or use \"Save to new slot\".";

/// What ties a test-slot mark to a character: 16 hex digits made from the character's class
/// and name. Both stay the same while the character is played; a different character under
/// the same file name gives a different mark. (Renaming a character changes it. Vault Key
/// carries the mark over when it does the renaming itself; after a rename in the game the
/// slot has to be marked again.)
pub fn character_mark(document: &SaveDocument) -> String {
    // FNV-1a, 64 bits. This is an identity check, not a security measure.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let class = document.class_path();
    let name = document.name();
    for byte in class.bytes().chain([0]).chain(name.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What identifies a protected character: 16 hex digits made from its class, name and
/// save-game ID. A copy made with "Save as a new character" gets a new save-game ID, so the
/// copy is not protected; the original stays protected in whichever slot it is.
pub fn protection_key(document: &SaveDocument) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let id = document.save_game_id().to_string();
    for byte in "protected\0".bytes().chain(document.class_path().bytes()).chain([0]).chain(document.name().bytes()).chain([0]).chain(id.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// True when the settings protect this character.
pub fn is_protected(settings: &Settings, document: &SaveDocument) -> bool {
    settings.is_protected(&protection_key(document))
}

/// The policy decision for a file whose contents are known: a protected character is
/// read-only whatever else the settings say; otherwise [`access`].
pub fn access_for(settings: &Settings, slot: &SlotId, document: Option<&SaveDocument>) -> WriteAccess {
    if document.is_some_and(|d| is_protected(settings, d)) {
        return WriteAccess::ReadOnly(PROTECTED_MESSAGE.to_string());
    }
    access(settings, slot, document.map(character_mark).as_deref())
}

/// The policy decision for one file. `character` is the [`character_mark`] of the character
/// that is in the file now, or `None` if the file cannot be read; a test-slot mark applies
/// only to the character it was made for.
pub fn access(settings: &Settings, slot: &SlotId, character: Option<&str>) -> WriteAccess {
    let Some(number) = slot.slot() else {
        return WriteAccess::ReadOnly(NOT_A_SLOT_MESSAGE.to_string());
    };
    // Vault Key only ever creates files in slots 2 to 9, so a "created" record for any
    // other slot is not one it wrote.
    if is_creatable_slot(number) && settings.is_created_by_app(slot) {
        return WriteAccess::Writable(WritableBecause::CreatedByApp);
    }
    match settings.test_slot_character(slot) {
        Some(marked) if character == Some(marked) => WriteAccess::Writable(WritableBecause::TestSlot),
        Some(_) if character.is_none() => WriteAccess::ReadOnly(UNREADABLE_MESSAGE.to_string()),
        Some(_) => WriteAccess::ReadOnly(OTHER_CHARACTER_MESSAGE.to_string()),
        None => WriteAccess::ReadOnly(READ_ONLY_MESSAGE.to_string()),
    }
}

/// True when Vault Key may create a new character file in this slot number.
pub(crate) fn is_creatable_slot(slot: u32) -> bool {
    (guards::MIN_SLOT..=guards::MAX_SLOT).contains(&slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAIGE: &str = "00000000000000a1";
    const OTHER: &str = "00000000000000b2";

    fn slot(file: &str) -> SlotId {
        SlotId::new("76561190000000000", file).unwrap()
    }

    #[test]
    fn nothing_is_writable_until_recorded() {
        let mut settings = Settings::default();
        for file in ["Save0001.sav", "Save0002.sav", "Save0009.sav", "Save000a.sav", "Save0002 - Copy.sav"] {
            assert!(!access(&settings, &slot(file), Some(GAIGE)).is_writable(), "{file}");
        }
        settings.record_created(&slot("Save0003.sav"));
        settings.mark_test_slot(&slot("Save0004.sav"), GAIGE);
        assert_eq!(
            access(&settings, &slot("save0003.SAV"), Some(OTHER)),
            WriteAccess::Writable(WritableBecause::CreatedByApp)
        );
        assert_eq!(access(&settings, &slot("Save0004.sav"), Some(GAIGE)), WriteAccess::Writable(WritableBecause::TestSlot));
        assert_eq!(
            access(&settings, &slot("Save0005.sav"), Some(GAIGE)),
            WriteAccess::ReadOnly(READ_ONLY_MESSAGE.to_string())
        );
        // The same file name under another Steam ID is another file.
        let other = SlotId::new("76561190000000001", "Save0003.sav").unwrap();
        assert!(!access(&settings, &other, Some(GAIGE)).is_writable());
    }

    #[test]
    fn a_test_slot_mark_covers_only_the_character_it_was_made_for() {
        let mut settings = Settings::default();
        settings.mark_test_slot(&slot("Save0001.sav"), GAIGE);
        // Slot 1 can be a test slot like any other.
        assert_eq!(access(&settings, &slot("SAVE0001.sav"), Some(GAIGE)), WriteAccess::Writable(WritableBecause::TestSlot));
        // Another character under the same file name, or a file that cannot be read: read-only.
        assert_eq!(
            access(&settings, &slot("Save0001.sav"), Some(OTHER)),
            WriteAccess::ReadOnly(OTHER_CHARACTER_MESSAGE.to_string())
        );
        assert_eq!(access(&settings, &slot("Save0001.sav"), None), WriteAccess::ReadOnly(UNREADABLE_MESSAGE.to_string()));
    }

    #[test]
    fn hand_edited_settings_grant_nothing_they_should_not() {
        let settings = Settings::parse(
            "created 76561190000000000/Save0001.sav\n\
             test-slot 76561190000000000/Save0001.sav\n\
             test-slot 76561190000000000/Save0004.sav character=XYZ\n\
             test-slot 76561190000000000/Save0002 - Copy.sav character=00000000000000a1\n",
        );
        // Vault Key never creates slot 1, so a "created" line for it is not believed; a
        // test-slot line without a character mark grants nothing.
        assert_eq!(
            access(&settings, &slot("Save0001.sav"), Some(GAIGE)),
            WriteAccess::ReadOnly(READ_ONLY_MESSAGE.to_string())
        );
        assert!(!access(&settings, &slot("Save0004.sav"), Some("XYZ")).is_writable());
        assert_eq!(
            access(&settings, &slot("Save0002 - Copy.sav"), Some(GAIGE)),
            WriteAccess::ReadOnly(NOT_A_SLOT_MESSAGE.to_string())
        );
    }

    #[test]
    fn a_protected_character_is_read_only_whatever_else_is_set() {
        let doc = crate::testutil::sample_doc(1);
        let mut settings = Settings::default();
        settings.record_created(&slot("Save0003.sav"));
        settings.mark_test_slot(&slot("Save0001.sav"), &character_mark(&doc));
        settings.mark_test_slot(&slot("Save0003.sav"), &character_mark(&doc));
        assert!(access_for(&settings, &slot("Save0001.sav"), Some(&doc)).is_writable());
        settings.protect(&protection_key(&doc), "Main");
        for file in ["Save0001.sav", "Save0003.sav"] {
            assert_eq!(access_for(&settings, &slot(file), Some(&doc)), WriteAccess::ReadOnly(PROTECTED_MESSAGE.to_string()));
        }
        // A copy with another save-game ID is not the protected character.
        let mut copy = doc.clone();
        copy.set_save_game_id(3).unwrap();
        assert!(access_for(&settings, &slot("Save0003.sav"), Some(&copy)).is_writable());
        // The protection survives being written and read back.
        let again = Settings::parse(&settings.to_text());
        assert!(is_protected(&again, &doc) && !is_protected(&again, &copy));
    }

    #[test]
    fn explanations_are_sentences() {
        let mut settings = Settings::default();
        settings.record_created(&slot("Save0003.sav"));
        assert!(access(&settings, &slot("Save0003.sav"), None).explanation().contains("created this character"));
        assert!(access(&settings, &slot("Save0005.sav"), None).explanation().contains("read-only"));
        assert!(is_creatable_slot(2) && is_creatable_slot(9) && !is_creatable_slot(1) && !is_creatable_slot(10));
    }
}
