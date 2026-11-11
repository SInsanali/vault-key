use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use super::cache::DatabaseCache;
use super::install::GameInstall;
use crate::database::ItemDatabase;
use crate::error::{GameDataError, Result};
use crate::extract::build_database;
use crate::package::UPackage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportPhase {
    Fingerprinting,
    /// Reading and decompressing the named package file.
    ReadingPackage(String),
    Building,
    Saving,
    Done,
}

/// How far an import has got. `fraction` runs from 0 to 1 and never decreases.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportProgress {
    pub phase: ImportPhase,
    pub fraction: f64,
}

/// Points in an import where tests step in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checkpoint {
    Parsed,
    Built,
}

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(GameDataError::Canceled)
    } else {
        Ok(())
    }
}

/// Returns the item database for `install`: the cached one when the cache holds a database
/// built from exactly these package files, otherwise a fresh import that is then cached.
///
/// `progress` is called as the work advances. Setting `cancel` (from another thread, or from
/// inside `progress`) stops the import with [`GameDataError::Canceled`]; a canceled import
/// writes nothing. The game install is only read.
pub fn load_or_import(
    install: &GameInstall,
    cache: &DatabaseCache,
    progress: &mut dyn FnMut(ImportProgress),
    cancel: &AtomicBool,
) -> Result<ItemDatabase> {
    run(install, Some(cache), progress, cancel, &mut |_| {})
}

/// Builds the item database from the install's packages without reading or writing any cache.
pub fn import(
    install: &GameInstall,
    progress: &mut dyn FnMut(ImportProgress),
    cancel: &AtomicBool,
) -> Result<ItemDatabase> {
    run(install, None, progress, cancel, &mut |_| {})
}

/// Progress bands: fingerprinting 0–0.10, reading 0.10–0.70 (by file size), building 0.70–0.95,
/// saving 0.95–1.
pub(crate) fn run(
    install: &GameInstall,
    cache: Option<&DatabaseCache>,
    progress: &mut dyn FnMut(ImportProgress),
    cancel: &AtomicBool,
    checkpoint: &mut dyn FnMut(Checkpoint),
) -> Result<ItemDatabase> {
    let mut report = |phase, fraction| progress(ImportProgress { phase, fraction });
    check(cancel)?;
    report(ImportPhase::Fingerprinting, 0.0);
    let fingerprint = install.fingerprint_unless(cancel)?;
    if let Some(hit) = cache.and_then(|c| c.load(&fingerprint)) {
        report(ImportPhase::Done, 1.0);
        return Ok(hit);
    }

    let sources: Vec<(PathBuf, f64)> = install
        .source_packages()
        .into_iter()
        .map(|path| {
            let size = std::fs::metadata(&path).map_or(1, |m| m.len().max(1));
            (path, size as f64)
        })
        .collect();
    let total: f64 = sources.iter().map(|(_, size)| size).sum();
    let mut done = 0.0;
    let mut packages = Vec::with_capacity(sources.len());
    let mut skipped = Vec::new();
    for (i, (path, size)) in sources.iter().enumerate() {
        check(cancel)?;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        report(
            ImportPhase::ReadingPackage(name.clone()),
            0.10 + 0.60 * done / total,
        );
        match UPackage::open(path) {
            Ok(package) => packages.push(package),
            // The base package is required. A DLC package that cannot be read is left out.
            Err(e) if i == 0 => return Err(e),
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
        done += size;
    }
    checkpoint(Checkpoint::Parsed);
    check(cancel)?;
    report(ImportPhase::Building, 0.70);
    let mut db = build_database(&packages, &fingerprint)?;
    skipped.append(&mut db.skipped_packages);
    db.skipped_packages = skipped;
    checkpoint(Checkpoint::Built);
    check(cancel)?;
    if let Some(cache) = cache {
        report(ImportPhase::Saving, 0.95);
        cache.store(&db)?;
    }
    report(ImportPhase::Done, 1.0);
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AssetCategory, DlcResolution, ItemRefs, PART_SLOTS};
    use crate::test_support::{base_world, chunked, dlc_world, whole_file_compressed, TempDir};
    use std::path::Path;

    /// An install whose base package uses the whole-file layout and whose DLC package uses the
    /// chunk-table layout, like the real game.
    fn game(tmp: &TempDir) -> GameInstall {
        let root = tmp.path().join("Game");
        let cooked = root.join("WillowGame").join("CookedPCConsole");
        std::fs::create_dir_all(&cooked).unwrap();
        let base = whole_file_compressed(&base_world().build(), 0x20000, 0x80000);
        std::fs::write(cooked.join("Startup.upk"), base).unwrap();
        add_dlc(&root, "Tulip", &chunked(&dlc_world(2).build(), 0x20000));
        GameInstall::new(root).unwrap()
    }

    fn add_dlc(root: &Path, name: &str, package: &[u8]) {
        let content = root.join("DLC").join(name).join("Compat").join("Content");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join(format!("{name}_Startup_SF.upk")), package).unwrap();
    }

    /// Every folder under `root`, and every file with its size and modification time. (Folder
    /// times are left out: NTFS refreshes the copy kept in the parent's index lazily, so they
    /// can appear to change when the folder is merely listed. A file added to or removed from
    /// a folder shows up in the listing itself.)
    fn snapshot(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let meta = std::fs::metadata(&path).unwrap();
                if meta.is_dir() {
                    out.push(format!("{}|dir", path.display()));
                    pending.push(path);
                } else {
                    out.push(format!(
                        "{}|{}|{:?}",
                        path.display(),
                        meta.len(),
                        meta.modified().unwrap()
                    ));
                }
            }
        }
        out.sort();
        out
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn imports_then_hits_the_cache() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        let cache = DatabaseCache::new(tmp.path().join("cache"), &install).unwrap();
        let before = snapshot(install.root());

        let mut seen = Vec::new();
        let db = load_or_import(&install, &cache, &mut |p| seen.push(p), &never()).unwrap();
        let phases: Vec<ImportPhase> = seen.iter().map(|p| p.phase.clone()).collect();
        assert_eq!(
            phases,
            [
                ImportPhase::Fingerprinting,
                ImportPhase::ReadingPackage("Startup.upk".into()),
                ImportPhase::ReadingPackage("Tulip_Startup_SF.upk".into()),
                ImportPhase::Building,
                ImportPhase::Saving,
                ImportPhase::Done,
            ]
        );
        assert_eq!(seen[0].fraction, 0.0);
        assert_eq!(seen[1].fraction, 0.10);
        assert!(seen[2].fraction > 0.10 && seen[2].fraction < 0.70);
        assert_eq!(seen.last().unwrap().fraction, 1.0);
        assert!(seen.windows(2).all(|w| w[0].fraction <= w[1].fraction));

        assert_eq!(db.fingerprint, install.fingerprint().unwrap());
        assert_eq!(db.sets.keys().copied().collect::<Vec<_>>(), [0, 2]);
        assert!(db.skipped_packages.is_empty());
        let pistol = ItemRefs {
            is_weapon: true,
            set: Some(0),
            item_type: Some(0),
            balance: Some(0),
            manufacturer: Some(0),
            parts: {
                let mut parts = [Some(131071); PART_SLOTS];
                parts[0] = Some(1); // Body4
                parts[2] = Some(2); // Barrel5
                parts[9] = Some(4);
                parts[10] = Some(5);
                parts
            },
        };
        let info = db.info(&pistol);
        assert_eq!(info.name.as_deref(), Some("Hot Shot"));
        assert_eq!(info.rarity.level, Some(9));
        assert_eq!(
            db.path_with(
                AssetCategory::WeaponParts,
                32 << 11,
                2,
                DlcResolution::Verified
            ),
            Some("GD_Tulip.Parts.BarrelSeraph")
        );

        // One cache file, and a second call is served from it without reading packages.
        assert_eq!(
            files(cache.directory()),
            [DatabaseCache::file_name(&db.fingerprint)]
        );
        let mut again = Vec::new();
        let cached =
            load_or_import(&install, &cache, &mut |p| again.push(p.phase), &never()).unwrap();
        assert_eq!(cached, db);
        assert_eq!(again, [ImportPhase::Fingerprinting, ImportPhase::Done]);

        // Nothing in the install was created, changed or removed.
        assert!(snapshot(install.root()) == before, "the install changed");
    }

    #[test]
    fn changed_packages_are_reimported() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        let cache = DatabaseCache::new(tmp.path().join("cache"), &install).unwrap();
        let first = load_or_import(&install, &cache, &mut |_| {}, &never()).unwrap();

        // A game update: another DLC appears.
        let mut other = dlc_world(5);
        other.object("GD_Tulip.Parts.Extra", "WeaponPartDefinition", &[]);
        add_dlc(install.root(), "Zinnia", &other.build());
        let mut phases = Vec::new();
        let second =
            load_or_import(&install, &cache, &mut |p| phases.push(p.phase), &never()).unwrap();
        assert_ne!(second.fingerprint, first.fingerprint);
        assert!(phases.contains(&ImportPhase::ReadingPackage("Zinnia_Startup_SF.upk".into())));
        assert_eq!(second.sets.keys().copied().collect::<Vec<_>>(), [0, 2, 5]);
        // The old fingerprint's cache file is gone.
        assert_eq!(
            files(cache.directory()),
            [DatabaseCache::file_name(&second.fingerprint)]
        );
    }

    #[test]
    fn import_without_a_cache_writes_nothing() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        let before = snapshot(tmp.path());
        let mut phases = Vec::new();
        let db = import(&install, &mut |p| phases.push(p.phase), &never()).unwrap();
        assert_eq!(db.sets.len(), 2);
        assert!(!phases.contains(&ImportPhase::Saving));
        assert_eq!(phases.last(), Some(&ImportPhase::Done));
        assert!(snapshot(tmp.path()) == before, "files changed");
    }

    #[test]
    fn unreadable_dlc_packages_are_skipped() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        add_dlc(install.root(), "Aster", b"this is not a package");
        let mut truncated = chunked(&dlc_world(9).build(), 0x20000);
        truncated.truncate(truncated.len() - 40);
        add_dlc(install.root(), "Sage", &truncated);
        let mut other_version = dlc_world(4).build();
        other_version[4] = 0x64;
        add_dlc(install.root(), "Zinnia", &other_version);

        let db = import(&install, &mut |_| {}, &never()).unwrap();
        assert_eq!(db.sets.keys().copied().collect::<Vec<_>>(), [0, 2]);
        assert_eq!(db.skipped_packages.len(), 3, "{:?}", db.skipped_packages);
        assert!(db.skipped_packages[0].starts_with("Aster_Startup_SF.upk: not a game package"));
        assert!(db.skipped_packages[1].starts_with("Sage_Startup_SF.upk: "));
        assert_eq!(
            db.skipped_packages[2],
            "Zinnia_Startup_SF.upk: unsupported package version 868, licensee 46"
        );
    }

    #[test]
    fn unreadable_base_package_fails_the_import() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        let cache = DatabaseCache::new(tmp.path().join("cache"), &install).unwrap();
        std::fs::write(install.startup_package(), b"garbage, not a package").unwrap();
        let result = load_or_import(&install, &cache, &mut |_| {}, &never());
        assert!(
            matches!(result, Err(GameDataError::NotAPackage(_))),
            "{result:?}"
        );
        assert!(files(cache.directory()).is_empty());

        // A valid package without the game's globals.
        std::fs::write(install.startup_package(), dlc_world(3).build()).unwrap();
        let result = load_or_import(&install, &cache, &mut |_| {}, &never());
        assert!(
            matches!(result, Err(GameDataError::UnexpectedData(_))),
            "{result:?}"
        );
        assert!(files(cache.directory()).is_empty());

        std::fs::remove_file(install.startup_package()).unwrap();
        let result = load_or_import(&install, &cache, &mut |_| {}, &never());
        assert!(
            matches!(result, Err(GameDataError::InstallNotFound(_))),
            "{result:?}"
        );
    }

    #[test]
    fn canceled_import_leaves_no_cache() {
        let tmp = TempDir::new();
        let install = game(&tmp);
        let cache = DatabaseCache::new(tmp.path().join("cache"), &install).unwrap();

        // Canceled before it starts: nothing is even reported.
        let mut calls = 0;
        let canceled = AtomicBool::new(true);
        let result = load_or_import(&install, &cache, &mut |_| calls += 1, &canceled);
        assert_eq!(result, Err(GameDataError::Canceled));
        assert_eq!(calls, 0);
        assert!(files(cache.directory()).is_empty());

        // Canceled from inside the progress callback, at each phase before the save.
        for stop_at in [
            ImportPhase::Fingerprinting,
            ImportPhase::ReadingPackage("Startup.upk".into()),
            ImportPhase::ReadingPackage("Tulip_Startup_SF.upk".into()),
            ImportPhase::Building,
        ] {
            let cancel = never();
            let mut progress = |p: ImportProgress| {
                if p.phase == stop_at {
                    cancel.store(true, Ordering::Relaxed);
                }
            };
            let result = load_or_import(&install, &cache, &mut progress, &cancel);
            assert_eq!(result, Err(GameDataError::Canceled), "{stop_at:?}");
            assert!(files(cache.directory()).is_empty(), "{stop_at:?}");
        }

        // Canceled right after parsing and right after building.
        for stage in [Checkpoint::Parsed, Checkpoint::Built] {
            let cancel = never();
            let mut checkpoint = |reached| {
                if reached == stage {
                    cancel.store(true, Ordering::Relaxed);
                }
            };
            let result = run(
                &install,
                Some(&cache),
                &mut |_| {},
                &cancel,
                &mut checkpoint,
            );
            assert_eq!(result, Err(GameDataError::Canceled), "{stage:?}");
            assert!(files(cache.directory()).is_empty(), "{stage:?}");
        }

        // The same install then imports normally.
        assert!(load_or_import(&install, &cache, &mut |_| {}, &never()).is_ok());
        assert_eq!(files(cache.directory()).len(), 1);
    }
}
