//! The character list: every save file in the SaveData folder, readable or not.

use std::fs;
use std::path::Path;

use savekit::SaveDocument;

use crate::error::{CoreError, ErrorKind, Result};
use crate::guards;
use crate::policy::{self, WriteAccess};
use crate::settings::Settings;
use crate::slot::SlotId;
use crate::time;

/// A playable class, from the class path stored in the save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharacterClass {
    Assassin,
    Commando,
    Siren,
    Gunzerker,
    Mechromancer,
    Psycho,
}

impl CharacterClass {
    /// Recognises a class path such as `GD_Assassin.Character.CharClass_Assassin` by its
    /// last part.
    pub fn from_class_path(path: &str) -> Option<CharacterClass> {
        match path.rsplit('.').next()? {
            "CharClass_Assassin" => Some(CharacterClass::Assassin),
            "CharClass_Soldier" => Some(CharacterClass::Commando),
            "CharClass_Siren" => Some(CharacterClass::Siren),
            "CharClass_Mercenary" => Some(CharacterClass::Gunzerker),
            "CharClass_Mechromancer" => Some(CharacterClass::Mechromancer),
            "CharClass_LilacPlayerClass" => Some(CharacterClass::Psycho),
            _ => None,
        }
    }

    /// The class name: "Mechromancer".
    pub fn label(self) -> &'static str {
        match self {
            CharacterClass::Assassin => "Assassin",
            CharacterClass::Commando => "Commando",
            CharacterClass::Siren => "Siren",
            CharacterClass::Gunzerker => "Gunzerker",
            CharacterClass::Mechromancer => "Mechromancer",
            CharacterClass::Psycho => "Psycho",
        }
    }

    /// The character's own name: "Gaige".
    pub fn hero_name(self) -> &'static str {
        match self {
            CharacterClass::Assassin => "Zer0",
            CharacterClass::Commando => "Axton",
            CharacterClass::Siren => "Maya",
            CharacterClass::Gunzerker => "Salvador",
            CharacterClass::Mechromancer => "Gaige",
            CharacterClass::Psycho => "Krieg",
        }
    }
}

/// The class name to show for a class path: a known class's name, else the last part of the
/// path, else "Unknown class".
pub fn class_label(class_path: &str) -> String {
    if let Some(class) = CharacterClass::from_class_path(class_path) {
        return class.label().to_string();
    }
    match class_path.rsplit('.').next().map(str::trim) {
        Some(last) if !last.is_empty() => last.to_string(),
        _ => "Unknown class".to_string(),
    }
}

/// One row of the character list. Plain data, ready to send to a front end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterEntry {
    pub slot_id: SlotId,
    /// The slot number, when the file has a standard slot name.
    pub slot: Option<u32>,
    pub steam_id: String,
    pub file_name: String,
    /// Full path of the file, for display.
    pub path: String,
    pub size: u64,
    /// Last-modified time in seconds since 1970-01-01 UTC, if the file system gave one.
    pub modified_unix: Option<u64>,
    /// The same moment as text, such as `2026-11-09 03:15:00 UTC`.
    pub modified_utc: Option<String>,
    /// `None` when the file decodes; otherwise why it does not. The character fields below
    /// are then empty or zero.
    pub problem: Option<String>,
    pub name: String,
    pub class_path: String,
    pub class: Option<CharacterClass>,
    pub class_label: String,
    pub level: i64,
    pub save_game_id: i64,
    /// Whether Vault Key may edit this file in place, and why or why not.
    pub access: WriteAccess,
    pub writable: bool,
    /// The character is protected: never written, whatever else is set.
    pub protected: bool,
    /// Steam Cloud syncs this folder (`steam_autocloud.vdf` is present): worth a warning,
    /// never a block.
    pub steam_cloud: bool,
}

impl CharacterEntry {
    pub fn is_readable(&self) -> bool {
        self.problem.is_none()
    }
}

/// The all-digit folders directly inside `root`, by name. An error if `root` cannot be listed.
pub(crate) fn steam_ids(root: &Path) -> Result<Vec<String>> {
    let entries = fs::read_dir(root).map_err(|e| {
        CoreError::new(
            ErrorKind::SaveFolder,
            format!("Cannot read the game's save folder {}: {e}", root.display()),
        )
    })?;
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    ids.sort();
    Ok(ids)
}

/// Save file names in one Steam-ID folder, by slot and then by name. `None` if the folder
/// cannot be listed.
pub(crate) fn save_file_names(folder: &Path) -> Option<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| guards::is_save_file_name(name))
        .collect();
    names.sort_by_key(|name| (guards::slot_from_file_name(name).unwrap_or(u32::MAX), name.to_lowercase()));
    Some(names)
}

/// Lists every save file under `root`. A file that cannot be read or decoded is still
/// listed, with [`CharacterEntry::problem`] set. An error only if `root` itself cannot be
/// listed.
pub fn scan(root: &Path, settings: &Settings) -> Result<Vec<CharacterEntry>> {
    let mut out = Vec::new();
    for steam_id in steam_ids(root)? {
        let folder = root.join(&steam_id);
        let steam_cloud = folder.join("steam_autocloud.vdf").is_file();
        for file_name in save_file_names(&folder).unwrap_or_default() {
            // A name that is not a plain save file name is not something to offer.
            let Ok(slot_id) = SlotId::new(&steam_id, &file_name) else { continue };
            out.push(describe(&folder.join(&file_name), slot_id, settings, steam_cloud));
        }
    }
    Ok(out)
}

fn describe(path: &Path, slot_id: SlotId, settings: &Settings, steam_cloud: bool) -> CharacterEntry {
    let meta = fs::metadata(path).ok();
    let modified_unix = meta.as_ref().and_then(|m| m.modified().ok()).map(time::unix_seconds);
    let decoded = match fs::read(path) {
        Ok(bytes) => SaveDocument::from_file_bytes(&bytes).map_err(|e| unreadable_reason(&e)),
        Err(e) => Err(format!("The file cannot be read: {e}")),
    };
    let (doc, problem) = match decoded {
        Ok(doc) => (Some(doc), None),
        Err(why) => (None, Some(why)),
    };
    let access = policy::access_for(settings, &slot_id, doc.as_ref());
    let protected = doc.as_ref().is_some_and(|d| policy::is_protected(settings, d));
    let class_path = doc.as_ref().map(SaveDocument::class_path).unwrap_or_default();
    CharacterEntry {
        slot: slot_id.slot(),
        steam_id: slot_id.steam_id().to_string(),
        file_name: slot_id.file_name().to_string(),
        path: path.display().to_string(),
        size: meta.as_ref().map_or(0, fs::Metadata::len),
        modified_unix,
        modified_utc: modified_unix.map(time::display_utc),
        problem,
        name: doc.as_ref().map(SaveDocument::name).unwrap_or_default(),
        class: CharacterClass::from_class_path(&class_path),
        class_label: if doc.is_some() { class_label(&class_path) } else { String::new() },
        class_path,
        level: doc.as_ref().map_or(0, SaveDocument::level),
        save_game_id: doc.as_ref().map_or(0, SaveDocument::save_game_id),
        writable: access.is_writable(),
        protected,
        access,
        steam_cloud,
        slot_id,
    }
}

/// Why a file is not a readable save, as a sentence.
pub(crate) fn unreadable_reason(error: &savekit::SaveKitError) -> String {
    let text = error.to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str().trim_end_matches('.')),
        None => "Not a readable save.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sample_bytes, sample_doc, TempDir, CLASS_PATH, STEAM_ID};
    use savekit::{field, ItemBlob, ProtoField, ProtoMessage, ProtoValue};

    #[test]
    fn classes_from_class_paths() {
        assert_eq!(
            CharacterClass::from_class_path("GD_Assassin.Character.CharClass_Assassin"),
            Some(CharacterClass::Assassin)
        );
        assert_eq!(
            CharacterClass::from_class_path("GD_Lilac_PlayerClass.Character.CharClass_LilacPlayerClass"),
            Some(CharacterClass::Psycho)
        );
        let commando = CharacterClass::from_class_path("GD_Soldier.Character.CharClass_Soldier").unwrap();
        assert_eq!((commando.label(), commando.hero_name()), ("Commando", "Axton"));
        assert_eq!(CharacterClass::from_class_path(CLASS_PATH), Some(CharacterClass::Mechromancer));
        assert_eq!(CharacterClass::from_class_path("Something.Else"), None);
        assert_eq!(class_label(CLASS_PATH), "Mechromancer");
        assert_eq!(class_label("Something.Else"), "Else");
        assert_eq!(class_label(""), "Unknown class");
    }

    /// A save the engine rejects as a Pre-Sequel save: a weapon whose blob has item version 10.
    fn pre_sequel_bytes() -> Vec<u8> {
        let mut doc = sample_doc(6);
        let blob = ItemBlob::new(true, 10, 5, vec![1, 2, 3, 4, 5, 6, 7, 8]).to_raw();
        let entry = ProtoMessage::new(vec![ProtoField::bytes(1, blob)]);
        doc.message.push_repeated(field::WEAPONS, ProtoValue::Bytes(entry.encode()));
        doc.to_file_bytes().unwrap()
    }

    #[test]
    fn lists_readable_and_unreadable_files_side_by_side() {
        let temp = TempDir::new();
        let root = temp.path().join("SaveData");
        let folder = root.join(STEAM_ID);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("Save0002.sav"), sample_bytes(2)).unwrap();
        fs::write(folder.join("Save0003.sav"), b"junk").unwrap();
        fs::write(folder.join("save000A.SAV"), sample_bytes(10)).unwrap();
        fs::write(folder.join("Save0005.sav"), pre_sequel_bytes()).unwrap();
        fs::write(folder.join("Save0002 - Copy.sav"), sample_bytes(2)).unwrap();
        fs::write(folder.join("notes.txt"), b"hello").unwrap();
        fs::write(folder.join("profile.bin"), b"profile").unwrap();
        fs::create_dir(folder.join("Save0007.sav")).unwrap();

        let mut settings = Settings::default();
        settings.record_created(&SlotId::new(STEAM_ID, "Save0002.sav").unwrap());
        let list = scan(&root, &settings).unwrap();
        let names: Vec<&str> = list.iter().map(|c| c.file_name.as_str()).collect();
        assert_eq!(names, ["Save0002.sav", "Save0003.sav", "Save0005.sav", "save000A.SAV", "Save0002 - Copy.sav"]);

        let good = &list[0];
        assert!(good.is_readable() && good.writable);
        assert_eq!((good.slot, good.name.as_str(), good.level, good.save_game_id), (Some(2), "Gaige", 5, 2));
        assert_eq!((good.class, good.class_label.as_str()), (Some(CharacterClass::Mechromancer), "Mechromancer"));
        assert_eq!(good.size, sample_bytes(2).len() as u64);
        assert!(good.modified_unix.unwrap() > 1_700_000_000);
        assert!(good.modified_utc.as_ref().unwrap().ends_with(" UTC"));
        assert!(good.path.ends_with("Save0002.sav"));
        assert!(!good.steam_cloud);

        let junk = &list[1];
        assert!(!junk.is_readable() && !junk.writable);
        assert!(junk.problem.as_ref().unwrap().starts_with("Not a Borderlands 2 save"), "{:?}", junk.problem);
        assert_eq!((junk.name.as_str(), junk.level, junk.size), ("", 0, 4));

        assert!(list[2].problem.as_ref().unwrap().contains("Pre-Sequel saves are not supported"));
        assert_eq!(list[3].slot, Some(10));
        assert_eq!((list[4].slot, list[4].writable), (None, false));
    }

    #[test]
    fn handles_missing_empty_and_several_steam_folders() {
        let temp = TempDir::new();
        let root = temp.path().join("SaveData");
        let settings = Settings::default();
        assert_eq!(scan(&root, &settings).unwrap_err().kind, ErrorKind::SaveFolder);

        fs::create_dir_all(&root).unwrap();
        assert!(scan(&root, &settings).unwrap().is_empty());

        for id in ["222", "111"] {
            fs::create_dir_all(root.join(id)).unwrap();
            fs::write(root.join(id).join("Save0002.sav"), sample_bytes(2)).unwrap();
        }
        fs::create_dir_all(root.join("backup-20261107-233403")).unwrap();
        fs::write(root.join("backup-20261107-233403").join("Save0001.sav"), sample_bytes(1)).unwrap();
        fs::write(root.join("333"), b"a file, not a folder").unwrap();
        fs::write(root.join("111").join("steam_autocloud.vdf"), b"").unwrap();

        let list = scan(&root, &settings).unwrap();
        let ids: Vec<&str> = list.iter().map(|c| c.steam_id.as_str()).collect();
        assert_eq!(ids, ["111", "222"]);
        assert!(list[0].steam_cloud && !list[1].steam_cloud);
        assert!(list.iter().all(|c| !c.writable), "nothing is writable by default");
    }
}
