//! The settings file: which slot files Vault Key may write.
//!
//! It is a small text file, one entry per line:
//!
//! ```text
//! # Vault Key settings
//! format 1
//! created 76561190000000000/Save0003.sav
//! test-slot 76561190000000000/Save0004.sav character=0123456789abcdef
//! ```
//!
//! `created` is a file this app made; `test-slot` is a slot the user marked as a test slot.
//! A test-slot mark is tied to the character that was in the slot when it was marked (see
//! [`crate::policy::character_mark`]): it grants nothing for a different character that later
//! turns up under the same file name. A `test-slot` line without a character grants nothing.
//! A line that is not understood grants nothing, and is kept as it is when the file is
//! written again (so a newer version's entries survive).
//!
//! A `save-folder <path>` line says which SaveData folder the entries are about. Slots are
//! identified by Steam ID and file name only, so entries made for one save folder (a test
//! sandbox, say) must never be applied to another (the real one). [`crate::Core`] refuses
//! to start on a settings file that belongs to a different save folder.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::{CoreError, ErrorKind, Result};
use crate::slot::SlotId;

pub const SETTINGS_FILE_NAME: &str = "settings.txt";

const HEADER: &str = "# Vault Key settings. One entry per line; lines that are not understood are kept.";
/// The header written before the app was renamed (it was called Golden Key until 0.9.0).
const OLD_HEADER: &str = "# Golden Key settings. One entry per line; lines that are not understood are kept.";
const FORMAT_LINE: &str = "format 1";
const SAVE_FOLDER: &str = "save-folder";
const CREATED: &str = "created";
const TEST_SLOT: &str = "test-slot";
const CHARACTER: &str = " character=";
const PROTECTED: &str = "protected";
const THEME: &str = "theme";

/// The app's color themes. The first is the default.
pub const THEMES: [&str; 4] = ["yellow", "blue", "purple", "red"];

/// A test-slot mark: the slot, and the character it was made for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TestMark {
    slot: SlotId,
    character: String,
}

/// A character mark as it is written: 16 lower-case hex digits.
fn is_character_mark(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What the settings file holds. Empty by default: nothing is writable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    /// The SaveData folder these entries are about, as [`save_folder_key`] writes it.
    save_folder: Option<String>,
    /// By [`SlotId::key`].
    created: BTreeMap<String, SlotId>,
    test_slots: BTreeMap<String, TestMark>,
    /// Protected characters: [`crate::policy::protection_key`] → a label for people.
    protected: BTreeMap<String, String>,
    /// One of [`THEMES`]; `None` for the default.
    theme: Option<String>,
    unknown_lines: Vec<String>,
}

/// How a SaveData folder is written in the settings file, so that two spellings of the same
/// folder compare equal: its physical location (links resolved), in lower case, without
/// Windows' `\\?\` prefix or a trailing separator.
pub fn save_folder_key(save_data_root: &Path) -> String {
    let physical = crate::guards::physical_path(save_data_root).to_string_lossy().to_lowercase();
    let plain = physical.strip_prefix("\\\\?\\").unwrap_or(&physical);
    plain.trim_end_matches(['\\', '/']).to_string()
}

impl Settings {
    /// Reads the settings file. A missing file gives the defaults; a file that exists but
    /// cannot be read is an error, so that it is not silently replaced by an empty one.
    pub fn load(path: &Path) -> Result<Settings> {
        match fs::read(path) {
            Ok(bytes) => Ok(Settings::parse(&String::from_utf8_lossy(&bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(CoreError::new(
                ErrorKind::Settings,
                format!("Cannot read the settings file {}: {e}", path.display()),
            )),
        }
    }

    /// Reads settings text. Never fails: a line that is not understood is kept aside.
    pub fn parse(text: &str) -> Settings {
        let mut settings = Settings::default();
        // Some editors put a byte-order mark at the start of a text file.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line == HEADER || line == OLD_HEADER || line == FORMAT_LINE {
                continue;
            }
            let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            if keyword == SAVE_FOLDER && !rest.trim().is_empty() && settings.save_folder.is_none() {
                settings.save_folder = Some(rest.trim().to_string());
                continue;
            }
            let rest = rest.trim();
            let understood = match keyword {
                CREATED => SlotId::parse(rest).ok().map(|slot| {
                    settings.created.insert(slot.key(), slot);
                }),
                TEST_SLOT => rest
                    .rsplit_once(CHARACTER)
                    .filter(|(_, character)| is_character_mark(character))
                    .and_then(|(slot, character)| Some((SlotId::parse(slot.trim()).ok()?, character)))
                    .map(|(slot, character)| {
                        settings.test_slots.insert(slot.key(), TestMark { slot, character: character.to_string() });
                    }),
                THEME => THEMES.contains(&rest).then(|| settings.theme = Some(rest.to_string())),
                PROTECTED => {
                    let (key, label) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    is_character_mark(key).then(|| {
                        settings.protected.insert(key.to_string(), label.trim().to_string());
                    })
                }
                _ => None,
            };
            if understood.is_none() {
                settings.unknown_lines.push(raw.trim_end().to_string());
            }
        }
        settings
    }

    /// The text of the settings file.
    pub fn to_text(&self) -> String {
        let mut out = format!("{HEADER}\n{FORMAT_LINE}\n");
        if let Some(folder) = &self.save_folder {
            out += &format!("{SAVE_FOLDER} {folder}\n");
        }
        for slot in self.created.values() {
            out += &format!("{CREATED} {slot}\n");
        }
        for mark in self.test_slots.values() {
            out += &format!("{TEST_SLOT} {}{CHARACTER}{}\n", mark.slot, mark.character);
        }
        for (key, label) in &self.protected {
            out += &format!("{PROTECTED} {key} {label}\n");
        }
        if let Some(theme) = &self.theme {
            out += &format!("{THEME} {theme}\n");
        }
        for line in &self.unknown_lines {
            out += line;
            out.push('\n');
        }
        out
    }

    /// Writes the settings file, replacing the old one in a single step.
    pub fn save(&self, path: &Path) -> Result<()> {
        let problem = |e: std::io::Error| {
            CoreError::new(ErrorKind::Settings, format!("Cannot write the settings file {}: {e}", path.display()))
        };
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder).map_err(problem)?;
        }
        let temp = path.with_extension("txt.tmp");
        let written = fs::File::create(&temp).and_then(|mut file| {
            file.write_all(self.to_text().as_bytes())?;
            file.sync_all()
        });
        let moved = written.and_then(|()| fs::rename(&temp, path));
        if moved.is_err() {
            let _ = fs::remove_file(&temp);
        }
        moved.map_err(problem)
    }

    /// The SaveData folder the entries are about, if the file says.
    pub fn save_folder(&self) -> Option<&str> {
        self.save_folder.as_deref()
    }

    pub(crate) fn set_save_folder(&mut self, key: String) {
        self.save_folder = Some(key);
    }

    pub fn is_created_by_app(&self, slot: &SlotId) -> bool {
        self.created.contains_key(&slot.key())
    }

    /// True when the slot carries a test-slot mark. Whether the mark applies to the character
    /// that is in the slot now is [`crate::policy::access`]'s question.
    pub fn is_test_slot(&self, slot: &SlotId) -> bool {
        self.test_slots.contains_key(&slot.key())
    }

    /// The character a slot's test-slot mark was made for, if the slot is marked.
    pub fn test_slot_character(&self, slot: &SlotId) -> Option<&str> {
        self.test_slots.get(&slot.key()).map(|mark| mark.character.as_str())
    }

    /// True when the character with this [`crate::policy::protection_key`] is protected.
    pub fn is_protected(&self, key: &str) -> bool {
        self.protected.contains_key(key)
    }

    /// The color theme: one of [`THEMES`].
    pub fn theme(&self) -> &str {
        self.theme.as_deref().unwrap_or(THEMES[0])
    }

    /// Sets the color theme. False (and nothing changes) for a name not in [`THEMES`].
    pub(crate) fn set_theme(&mut self, theme: &str) -> bool {
        if !THEMES.contains(&theme) {
            return false;
        }
        self.theme = (theme != THEMES[0]).then(|| theme.to_string());
        true
    }

    /// Protected characters: (key, label).
    pub fn protected(&self) -> Vec<(String, String)> {
        self.protected.iter().map(|(k, l)| (k.clone(), l.clone())).collect()
    }

    pub(crate) fn protect(&mut self, key: &str, label: &str) {
        let label: String = label.chars().filter(|c| !c.is_control()).collect();
        self.protected.insert(key.to_string(), label);
    }

    /// Takes the protection off the character with this key. True if it was protected.
    pub(crate) fn unprotect(&mut self, key: &str) -> bool {
        self.protected.remove(key).is_some()
    }

    /// Files this app created, in order.
    pub fn created_slots(&self) -> Vec<SlotId> {
        self.created.values().cloned().collect()
    }

    /// Slots the user marked as test slots, in order.
    pub fn test_slots(&self) -> Vec<SlotId> {
        self.test_slots.values().map(|mark| mark.slot.clone()).collect()
    }

    pub(crate) fn record_created(&mut self, slot: &SlotId) {
        self.created.insert(slot.key(), slot.clone());
    }

    /// Marks the slot as a test slot for the character with this mark.
    pub(crate) fn mark_test_slot(&mut self, slot: &SlotId, character: &str) {
        self.test_slots.insert(slot.key(), TestMark { slot: slot.clone(), character: character.to_string() });
    }

    /// True if the slot was marked.
    pub(crate) fn unmark_test_slot(&mut self, slot: &SlotId) -> bool {
        self.test_slots.remove(&slot.key()).is_some()
    }

    /// Drops every record of the slot. True if there was one.
    pub(crate) fn forget(&mut self, slot: &SlotId) -> bool {
        let key = slot.key();
        let created = self.created.remove(&key).is_some();
        let marked = self.test_slots.remove(&key).is_some();
        created || marked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn a_settings_file_from_before_the_rename_reads_without_leftovers() {
        let settings = Settings::parse(&format!("{OLD_HEADER}\n{FORMAT_LINE}\ntheme blue\n"));
        assert_eq!(settings.theme(), "blue");
        let text = settings.to_text();
        assert!(text.starts_with(HEADER) && !text.contains("Golden Key"), "{text}");
    }

    #[test]
    fn the_theme_is_kept_and_unknown_names_are_refused() {
        let mut settings = Settings::default();
        assert_eq!(settings.theme(), "yellow");
        assert!(settings.set_theme("purple"));
        assert!(!settings.set_theme("green"));
        let again = Settings::parse(&settings.to_text());
        assert_eq!(again.theme(), "purple");
        // The default is not written; an unknown name in the file is kept as it was.
        assert!(settings.set_theme("yellow"));
        assert!(!settings.to_text().contains("theme"));
        let odd = Settings::parse("theme green\n");
        assert_eq!((odd.theme(), odd.to_text().contains("theme green")), ("yellow", true));
    }

    fn slot(file: &str) -> SlotId {
        SlotId::new("76561190000000000", file).unwrap()
    }

    #[test]
    fn nothing_is_writable_by_default() {
        let settings = Settings::default();
        assert!(!settings.is_created_by_app(&slot("Save0002.sav")));
        assert!(!settings.is_test_slot(&slot("Save0002.sav")));
        assert_eq!(Settings::parse(""), settings);
    }

    #[test]
    fn round_trips_and_ignores_letter_case() {
        let mut settings = Settings::default();
        settings.record_created(&slot("Save0003.sav"));
        settings.mark_test_slot(&slot("Save0004.sav"), "0123456789abcdef");
        let again = Settings::parse(&settings.to_text());
        assert_eq!(again, settings);
        assert!(again.is_created_by_app(&slot("SAVE0003.SAV")));
        assert!(again.is_test_slot(&slot("save0004.sav")));
        assert_eq!(again.test_slot_character(&slot("save0004.sav")), Some("0123456789abcdef"));
        assert!(!again.is_test_slot(&slot("Save0003.sav")));
        assert_eq!(again.test_slot_character(&slot("Save0003.sav")), None);
        assert!(!again.is_created_by_app(&SlotId::new("76561190000000001", "Save0003.sav").unwrap()));
    }

    #[test]
    fn unknown_and_damaged_lines_grant_nothing_and_are_kept() {
        let text = "format 1\r\n\
                    theme comic\r\n\
                    created 76561190000000000/Save0003.sav\r\n\
                    created ../../elsewhere/Save0004.sav\n\
                    created\n\
                    test-slot 76561190000000000/profile.bin character=0123456789abcdef\n\
                    test-slot 76561190000000000/Save0006.sav\n\
                    test-slot 76561190000000000/Save0007.sav character=0123456789ABCDEF\n\
                    test-slot 76561190000000000/Save0008.sav character=123\n\
                    allow-everything yes\n\
                    \u{feff}created 76561190000000000/Save0005.sav\n";
        let settings = Settings::parse(text);
        assert_eq!(settings.created_slots(), vec![slot("Save0003.sav")]);
        assert!(settings.test_slots().is_empty());
        assert_eq!(settings.unknown_lines.len(), 9);
        // Written again, the unknown lines are still there, and still grant nothing.
        let again = Settings::parse(&settings.to_text());
        assert_eq!(again, settings);
        assert!(settings.to_text().contains("theme comic\n"));
        assert!(settings.to_text().contains("allow-everything yes\n"));
    }

    #[test]
    fn the_save_folder_line_round_trips() {
        let mut settings = Settings::default();
        assert_eq!(settings.save_folder(), None);
        settings.set_save_folder("c:\\users\\me\\documents\\my games\\borderlands 2\\willowgame\\savedata".to_string());
        settings.record_created(&slot("Save0003.sav"));
        let again = Settings::parse(&settings.to_text());
        assert_eq!(again, settings);
        assert!(again.save_folder().unwrap().ends_with("my games\\borderlands 2\\willowgame\\savedata"));
        // Only the first such line counts; a second one is kept but means nothing.
        let two = Settings::parse("save-folder a\nsave-folder b\n");
        assert_eq!(two.save_folder(), Some("a"));
        assert_eq!(two.unknown_lines, ["save-folder b"]);
    }

    #[test]
    fn save_folder_keys_ignore_spelling() {
        let temp = TempDir::new();
        let root = temp.path().join("SaveData");
        // The same key whether or not the folder exists yet, and whatever the letter case.
        let before = save_folder_key(&root);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(save_folder_key(&root), before);
        let shouted = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
        assert_eq!(save_folder_key(&shouted), before);
        assert_eq!(save_folder_key(&root.join("x").join("..")), before);
        assert!(!before.starts_with("\\\\?\\") && !before.ends_with('\\'));
        assert_ne!(save_folder_key(&temp.path().join("SaveData2")), before);
    }

    #[test]
    fn garbage_parses_to_the_defaults() {
        let garbage = String::from_utf8_lossy(&[0xFF, 0xFE, 0x00, 0x41, 0x0A, 0x80]).into_owned();
        let settings = Settings::parse(&garbage);
        assert!(settings.created_slots().is_empty() && settings.test_slots().is_empty());
    }

    #[test]
    fn save_and_load() {
        let temp = TempDir::new();
        let path = temp.path().join("VaultKey").join(SETTINGS_FILE_NAME);
        assert_eq!(Settings::load(&path).unwrap(), Settings::default(), "a missing file is the defaults");
        let mut settings = Settings::default();
        settings.record_created(&slot("Save0003.sav"));
        settings.save(&path).unwrap();
        settings.mark_test_slot(&slot("Save0004.sav"), "0123456789abcdef");
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        assert!(settings.forget(&slot("SAVE0003.sav")));
        assert!(!settings.forget(&slot("Save0003.sav")));
        assert!(settings.unmark_test_slot(&slot("Save0004.sav")));
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names.len(), 1, "no temp file left: {names:?}");
    }

    #[test]
    fn a_settings_path_that_cannot_be_read_or_written_is_an_error() {
        let temp = TempDir::new();
        // A folder where the file should be.
        let path = temp.path().join(SETTINGS_FILE_NAME);
        fs::create_dir(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap_err().kind, ErrorKind::Settings);
        assert_eq!(Settings::default().save(&path).unwrap_err().kind, ErrorKind::Settings);
    }
}
