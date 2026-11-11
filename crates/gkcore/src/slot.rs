//! How a character file is identified: the Steam-ID folder plus the file name.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::error::{fail, ErrorKind, Result};
use crate::guards;

/// One character file in the SaveData folder: `<steam_id>/<file_name>`.
///
/// Two ids are equal when they differ only in letter case, because Windows file names are
/// case-insensitive.
#[derive(Debug, Clone)]
pub struct SlotId {
    steam_id: String,
    file_name: String,
}

impl SlotId {
    /// Checks both parts: the Steam ID is all digits, and the file name is a plain
    /// `Save*.sav` name with nothing that could point somewhere else.
    pub fn new(steam_id: &str, file_name: &str) -> Result<SlotId> {
        if steam_id.is_empty() || steam_id.len() > 32 || !steam_id.bytes().all(|b| b.is_ascii_digit()) {
            return fail(ErrorKind::InvalidTarget, format!("\"{steam_id}\" is not a Steam-ID folder name (digits only)."));
        }
        let plain = !file_name.is_empty()
            && file_name.len() <= 64
            && file_name.chars().all(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            && !file_name.contains("..");
        if !plain || !guards::is_save_file_name(file_name) {
            return fail(ErrorKind::InvalidTarget, format!("\"{file_name}\" is not a save file name (like Save0002.sav)."));
        }
        Ok(SlotId { steam_id: steam_id.to_string(), file_name: file_name.to_string() })
    }

    /// The id for slot `slot` under its standard file name.
    pub fn for_slot(steam_id: &str, slot: u32) -> Result<SlotId> {
        SlotId::new(steam_id, &guards::slot_file_name(slot))
    }

    /// Parses the `<steam_id>/<file_name>` form that [`fmt::Display`] writes.
    pub fn parse(text: &str) -> Result<SlotId> {
        match text.split_once('/') {
            Some((steam_id, file_name)) => SlotId::new(steam_id, file_name),
            None => fail(ErrorKind::InvalidTarget, format!("\"{text}\" is not a slot id (expected <steam id>/<file name>).")),
        }
    }

    pub fn steam_id(&self) -> &str {
        &self.steam_id
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// The slot number, when the file has a standard slot name (`Save` + 4 digits + `.sav`).
    pub fn slot(&self) -> Option<u32> {
        guards::slot_from_file_name(&self.file_name)
    }

    /// The lower-case form used for comparing and for the settings file.
    pub fn key(&self) -> String {
        format!("{}/{}", self.steam_id, self.file_name.to_lowercase())
    }

    /// Where this file is, inside the SaveData folder `root`.
    pub fn path_in(&self, root: &Path) -> PathBuf {
        root.join(&self.steam_id).join(&self.file_name)
    }
}

impl PartialEq for SlotId {
    fn eq(&self, other: &SlotId) -> bool {
        self.key() == other.key()
    }
}

impl Eq for SlotId {}

impl Hash for SlotId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl fmt::Display for SlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.steam_id, self.file_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_compare_without_letter_case() {
        let a = SlotId::new("76561190000000000", "Save0003.sav").unwrap();
        let b = SlotId::new("76561190000000000", "SAVE0003.SAV").unwrap();
        let c = SlotId::new("76561190000000001", "Save0003.sav").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.key(), "76561190000000000/save0003.sav");
        assert_eq!(a.to_string(), "76561190000000000/Save0003.sav");
        assert_eq!(SlotId::parse(&b.to_string()).unwrap(), a);
        assert_eq!(a.slot(), Some(3));
        assert_eq!(SlotId::for_slot("1", 9).unwrap().file_name(), "Save0009.sav");
        assert_eq!(SlotId::new("1", "Save0003 - Copy.sav").unwrap().slot(), None);
    }

    #[test]
    fn ids_that_could_point_elsewhere_are_rejected() {
        for (steam, file) in [
            ("", "Save0003.sav"),
            ("abc", "Save0003.sav"),
            ("..", "Save0003.sav"),
            ("123/456", "Save0003.sav"),
            ("123", ""),
            ("123", "profile.bin"),
            ("123", "Save0003.sav.bak"),
            ("123", "..\\Save0003.sav"),
            ("123", "Save/../../x.sav"),
            ("123", "Save0003.sav:stream.sav"),
            ("123", "Save\n0003.sav"),
            ("123", "Save..sav"),
        ] {
            assert!(SlotId::new(steam, file).is_err(), "{steam:?} {file:?}");
        }
        assert!(SlotId::parse("no-separator").is_err());
        assert!(SlotId::parse("123/456/Save0003.sav").is_err());
    }
}
