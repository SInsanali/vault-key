use std::path::{Path, PathBuf};

use super::install::GameInstall;
use super::path_guard;
use crate::database::ItemDatabase;
use crate::error::{GameDataError, Result};

const FILE_PREFIX: &str = "items-";
const FILE_SUFFIX: &str = ".gkdb";

/// The cache of the built database: one file in a folder outside the game install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseCache {
    directory: PathBuf,
}

impl DatabaseCache {
    /// `%LOCALAPPDATA%\VaultKey\GameData`, or `None` when `LOCALAPPDATA` is not set.
    pub fn default_directory() -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
        Some(PathBuf::from(local).join("VaultKey").join("GameData"))
    }

    /// Uses `directory` for the cache, creating it if needed. A location inside the game
    /// install (directly, through a link, or in different letter case) is refused with
    /// [`GameDataError::CacheLocationInsideGame`] before anything is created.
    pub fn new(directory: impl Into<PathBuf>, install: &GameInstall) -> Result<Self> {
        let directory = directory.into();
        if path_guard::is_inside(&directory, install.root()) {
            return Err(GameDataError::CacheLocationInsideGame);
        }
        std::fs::create_dir_all(&directory).map_err(|e| unreadable(&directory, &e))?;
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The cache file's name for a fingerprint: `items-<its first 16 characters>.gkdb`.
    pub fn file_name(fingerprint: &str) -> String {
        let short: String = fingerprint
            .chars()
            .take(16)
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{FILE_PREFIX}{short}{FILE_SUFFIX}")
    }

    /// The cached database for this fingerprint. `None` when the file is missing, damaged,
    /// from another schema version, or for another fingerprint.
    pub fn load(&self, fingerprint: &str) -> Option<ItemDatabase> {
        let bytes = std::fs::read(self.directory.join(Self::file_name(fingerprint))).ok()?;
        let db = ItemDatabase::from_bytes(&bytes).ok()?;
        (db.schema == ItemDatabase::SCHEMA_VERSION && db.fingerprint == fingerprint).then_some(db)
    }

    /// Writes the database (to a temporary file that then replaces the real one, so a reader
    /// never sees a half-written cache) and removes the cache files of other fingerprints.
    pub fn store(&self, db: &ItemDatabase) -> Result<()> {
        let name = Self::file_name(&db.fingerprint);
        let target = self.directory.join(&name);
        let temporary = self
            .directory
            .join(format!("{name}.{}.tmp", std::process::id()));
        let written = std::fs::write(&temporary, db.to_bytes())
            .and_then(|()| std::fs::rename(&temporary, &target));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(unreadable(&self.directory, &e));
        }
        let entries =
            std::fs::read_dir(&self.directory).map_err(|e| unreadable(&self.directory, &e))?;
        for entry in entries.flatten() {
            let other = entry.file_name();
            let other = other.to_string_lossy();
            if other != name.as_str()
                && other.starts_with(FILE_PREFIX)
                && other.ends_with(FILE_SUFFIX)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

fn unreadable(directory: &Path, error: &std::io::Error) -> GameDataError {
    GameDataError::CacheUnreadable(format!("{}: {error}", directory.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{link_dir, synthetic_install, TempDir};

    fn cache_files(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn cache_inside_game_folder_rejected() {
        let tmp = TempDir::new();
        let root = synthetic_install(&tmp.path().join("Game"));
        let install = GameInstall::new(&root).unwrap();
        let inside = [
            root.clone(),
            root.join("x"),
            root.join("WillowGame")
                .join("CookedPCConsole")
                .join("cache"),
            root.join("brand").join("new").join("sub"),
            PathBuf::from(root.to_string_lossy().to_uppercase()).join("NEW"),
            root.join("WillowGame").join("..").join("y"),
        ];
        for directory in &inside {
            assert_eq!(
                DatabaseCache::new(directory, &install),
                Err(GameDataError::CacheLocationInsideGame),
                "{}",
                directory.display()
            );
        }
        // Refusing created nothing.
        for name in ["x", "brand", "NEW", "y"] {
            assert!(!root.join(name).exists(), "{name}");
        }
        assert!(!root
            .join("WillowGame")
            .join("CookedPCConsole")
            .join("cache")
            .exists());

        let outside = tmp.path().join("outside").join("cache");
        let cache = DatabaseCache::new(&outside, &install).unwrap();
        assert!(outside.is_dir());
        assert_eq!(cache.directory(), outside);
    }

    #[test]
    fn cache_guard_resists_links() {
        let tmp = TempDir::new();
        let root = synthetic_install(&tmp.path().join("Game"));
        let install = GameInstall::new(&root).unwrap();
        let link = tmp.path().join("link");
        let link2 = tmp.path().join("link2");
        if !link_dir(&root, &link) || !link_dir(&root.join("WillowGame"), &link2) {
            eprintln!("skipped: cannot create directory links here");
            return;
        }
        for directory in [
            link.clone(),
            link.join("newdir").join("deeper"),
            link2.join("..").join("new"),
        ] {
            assert_eq!(
                DatabaseCache::new(&directory, &install),
                Err(GameDataError::CacheLocationInsideGame),
                "{}",
                directory.display()
            );
        }
        assert!(!root.join("newdir").exists());
        assert!(!root.join("new").exists());
    }

    #[test]
    fn stale_or_corrupt_cache_is_ignored() {
        let tmp = TempDir::new();
        let install = GameInstall::new(synthetic_install(&tmp.path().join("Game"))).unwrap();
        let directory = tmp.path().join("cache");
        let cache = DatabaseCache::new(&directory, &install).unwrap();
        let (fa, fb) = ("aa".repeat(32), "bb".repeat(32));
        let db = ItemDatabase::new(fa.clone());

        assert_eq!(cache.load(&fa), None);
        cache.store(&db).unwrap();
        assert_eq!(cache.load(&fa), Some(db.clone()));
        assert_eq!(cache.load(&fb), None);

        // Same file name, different full fingerprint.
        let near = format!("{}{}", &fa[..16], "c".repeat(48));
        assert_eq!(
            DatabaseCache::file_name(&near),
            DatabaseCache::file_name(&fa)
        );
        assert_eq!(cache.load(&near), None);

        let file = directory.join(DatabaseCache::file_name(&fa));
        std::fs::write(&file, "{").unwrap();
        assert_eq!(cache.load(&fa), None);
        let full = db.to_bytes();
        std::fs::write(&file, &full[..full.len() / 2]).unwrap(); // half-written
        assert_eq!(cache.load(&fa), None);

        let mut old = db.clone();
        old.schema = 1;
        cache.store(&old).unwrap();
        assert_eq!(cache.load(&fa), None);

        // Storing replaces the file and removes other fingerprints' files, leaving exactly one.
        cache.store(&db).unwrap();
        assert_eq!(cache.load(&fa), Some(db));
        cache.store(&ItemDatabase::new(fb.clone())).unwrap();
        assert_eq!(cache_files(&directory), [DatabaseCache::file_name(&fb)]);
        assert_eq!(cache.load(&fa), None);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(
            DatabaseCache::file_name(&"0123456789abcdef".repeat(4)),
            "items-0123456789abcdef.gkdb"
        );
        assert_eq!(DatabaseCache::file_name("../..\\x"), "items-______x.gkdb");
        assert_eq!(DatabaseCache::file_name(""), "items-.gkdb");
    }
}
