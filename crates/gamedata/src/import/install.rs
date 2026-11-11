use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::sha256::{hex, Sha256};
use crate::database::ItemDatabase;
use crate::error::{GameDataError, Result};

/// The environment variable that names the install folder, overriding discovery.
pub const GAME_DATA_ENV: &str = "GK_GAME_DATA";

const STARTUP_RELATIVE: &str = "WillowGame/CookedPCConsole/Startup.upk";
const DEFAULT_INSTALL_DIR: &str = "Borderlands 2";

/// A Borderlands 2 install: the folder that holds `WillowGame` and `DLC`. Everything here only
/// reads; nothing is ever created, changed or deleted under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInstall {
    root: PathBuf,
}

impl GameInstall {
    /// Borderlands 2's Steam app id.
    pub const STEAM_APP_ID: u32 = 49520;

    /// Accepts `root` when `WillowGame/CookedPCConsole/Startup.upk` under it is a readable file.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let startup = root.join(STARTUP_RELATIVE);
        if startup.is_file() && File::open(&startup).is_ok() {
            Ok(Self { root })
        } else {
            Err(GameDataError::InstallNotFound(root.display().to_string()))
        }
    }

    /// The install folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Finds the install. A non-empty `GK_GAME_DATA` environment variable names the install
    /// folder and is the only place looked at. Otherwise the default Steam folders are searched
    /// with [`GameInstall::locate_in`].
    pub fn locate() -> Option<Self> {
        Self::locate_from(
            std::env::var_os(GAME_DATA_ENV),
            &Self::default_steam_roots(),
        )
    }

    /// [`GameInstall::locate`] with its inputs spelled out: the override, if any, and the Steam
    /// folders to search otherwise.
    fn locate_from(override_path: Option<OsString>, steam_roots: &[PathBuf]) -> Option<Self> {
        match override_path {
            Some(path) if !path.is_empty() => Self::new(PathBuf::from(path)).ok(),
            _ => steam_roots.iter().find_map(|root| Self::locate_in(root)),
        }
    }

    /// Where Steam itself is installed by default: `Steam` under the 32-bit and 64-bit program
    /// folders.
    pub fn default_steam_roots() -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let candidates = ["ProgramFiles(x86)", "ProgramFiles"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .chain([PathBuf::from(r"C:\Program Files (x86)")]);
        for folder in candidates {
            let root = folder.join("Steam");
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }

    /// Looks for the game in a Steam folder's own library, then in every extra library listed
    /// by `"path"` entries in its `steamapps/libraryfolders.vdf`.
    ///
    /// In each library the game is at `steamapps/common/<installdir>`, where `<installdir>`
    /// comes from that library's `appmanifest_49520.acf` when present and is `Borderlands 2`
    /// otherwise. (The `"apps"` lists inside `libraryfolders.vdf` are not used: they can be out
    /// of date.)
    pub fn locate_in(steam_root: &Path) -> Option<Self> {
        let mut libraries = vec![steam_root.to_path_buf()];
        let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read(vdf) {
            let text = String::from_utf8_lossy(&text);
            libraries.extend(vdf_values(&text, "path").into_iter().map(PathBuf::from));
        }
        libraries.iter().find_map(|library| {
            let steamapps = library.join("steamapps");
            let manifest = steamapps.join(format!("appmanifest_{}.acf", Self::STEAM_APP_ID));
            let mut names: Vec<String> = std::fs::read(manifest)
                .map(|text| vdf_values(&String::from_utf8_lossy(&text), "installdir"))
                .unwrap_or_default();
            names.push(DEFAULT_INSTALL_DIR.to_string());
            names
                .iter()
                .find_map(|name| Self::new(steamapps.join("common").join(name)).ok())
        })
    }

    /// The base game's startup package, which holds every base-game definition.
    pub fn startup_package(&self) -> PathBuf {
        self.root.join(STARTUP_RELATIVE)
    }

    /// The names of the folders under `DLC`, sorted.
    fn dlc_folders(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.join("DLC"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        names
    }

    /// Every `DLC/<Name>/Compat/Content/<Name>_Startup_SF.upk` that exists, sorted by name. A
    /// missing `DLC` folder gives an empty list.
    pub fn dlc_startup_packages(&self) -> Vec<PathBuf> {
        self.dlc_folders()
            .iter()
            .map(|name| {
                self.root
                    .join("DLC")
                    .join(name)
                    .join("Compat")
                    .join("Content")
                    .join(format!("{name}_Startup_SF.upk"))
            })
            .filter(|path| path.is_file())
            .collect()
    }

    /// DLC folder name → the `PackageId` in its `Compat/Config/WillowDlc.ini` (the first
    /// `+PackageId=` line; the ini repeats it in every section). A folder whose ini is missing
    /// or has no such line is left out.
    pub fn dlc_package_ids(&self) -> BTreeMap<String, u32> {
        self.dlc_folders()
            .into_iter()
            .filter_map(|name| {
                let ini = self
                    .root
                    .join("DLC")
                    .join(&name)
                    .join("Compat/Config/WillowDlc.ini");
                let id = first_package_id(&std::fs::read(ini).ok()?)?;
                Some((name, id))
            })
            .collect()
    }

    /// The packages the database is built from: the base startup package, then the DLC ones.
    pub fn source_packages(&self) -> Vec<PathBuf> {
        let mut packages = vec![self.startup_package()];
        packages.extend(self.dlc_startup_packages());
        packages
    }

    /// A path relative to the install, with `/` separators.
    fn relative_path(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Identifies the source packages' contents: hex SHA-256 over the database schema version
    /// and, for each source package in relative-path order, its path, size and content hash.
    /// A changed, added or removed package (a game update, a new DLC) changes the fingerprint.
    pub fn fingerprint(&self) -> Result<String> {
        self.fingerprint_unless(&AtomicBool::new(false))
    }

    /// [`GameInstall::fingerprint`], stopping with [`GameDataError::Canceled`] once `cancel`
    /// is set.
    pub(crate) fn fingerprint_unless(&self, cancel: &AtomicBool) -> Result<String> {
        let mut packages: Vec<(String, PathBuf)> = self
            .source_packages()
            .into_iter()
            .map(|p| (self.relative_path(&p), p))
            .collect();
        packages.sort();
        let mut hasher = Sha256::new();
        hasher.update(format!("schema {}\n", ItemDatabase::SCHEMA_VERSION).as_bytes());
        let mut buffer = vec![0u8; 1 << 20];
        for (relative, path) in packages {
            let missing = |_| GameDataError::InstallNotFound(path.display().to_string());
            let mut file = File::open(&path).map_err(missing)?;
            let size = file.metadata().map_err(missing)?.len();
            hasher.update(format!("{relative}\n{size}\n").as_bytes());
            let mut contents = Sha256::new();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(GameDataError::Canceled);
                }
                let n = file.read(&mut buffer).map_err(missing)?;
                if n == 0 {
                    break;
                }
                contents.update(&buffer[..n]);
            }
            hasher.update(&contents.finalize());
        }
        Ok(hex(&hasher.finalize()))
    }
}

/// The values of every `"key" "value"` line in Valve's text format (`.vdf`, `.acf`), with the
/// format's backslash escapes undone. Keys compare without regard to case.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut tokens = quoted_tokens(line);
            let found = tokens.next()?;
            let value = tokens.next()?;
            found.eq_ignore_ascii_case(key).then_some(value)
        })
        .collect()
}

/// The double-quoted strings on one line. `\\` is a backslash and `\"` a quote.
fn quoted_tokens(line: &str) -> impl Iterator<Item = String> + '_ {
    let mut chars = line.chars();
    std::iter::from_fn(move || {
        chars.find(|&c| c == '"')?;
        let mut token = String::new();
        loop {
            match chars.next()? {
                '"' => return Some(token),
                '\\' => token.push(chars.next()?),
                c => token.push(c),
            }
        }
    })
}

/// The number on the first `+PackageId=` line of a `WillowDlc.ini`.
fn first_package_id(ini: &[u8]) -> Option<u32> {
    let text = if let Some(utf16) = ini.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = utf16
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(ini).into_owned()
    };
    text.lines()
        .find_map(|line| line.trim().strip_prefix("+PackageId=")?.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{synthetic_install, TempDir};

    #[test]
    fn missing_install_is_install_not_found() {
        let tmp = TempDir::new();
        assert!(matches!(
            GameInstall::new(tmp.path()),
            Err(GameDataError::InstallNotFound(_))
        ));
        // A folder where the file should be is not an install either.
        std::fs::create_dir_all(tmp.path().join(STARTUP_RELATIVE)).unwrap();
        assert!(matches!(
            GameInstall::new(tmp.path()),
            Err(GameDataError::InstallNotFound(_))
        ));
    }

    #[test]
    fn reads_valve_text_values() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"PATH\"\t\t\"D:\\\\Games \\\"2\\\"\"\n\t}\n}\n";
        assert_eq!(
            vdf_values(text, "path"),
            [
                r"C:\Program Files (x86)\Steam".to_string(),
                "D:\\Games \"2\"".to_string()
            ]
        );
        assert_eq!(vdf_values(text, "label"), [String::new()]);
        assert!(vdf_values(text, "missing").is_empty());
        // Unterminated and odd lines are ignored.
        assert!(vdf_values("\"path\" \"C:\\\n\"path\"\n{ } path", "path").is_empty());
    }

    #[test]
    fn locate_reads_library_folders_and_app_manifest() {
        let tmp = TempDir::new();
        let steam = tmp.path().join("Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        // The default folder name in a second library.
        let lib2 = tmp.path().join("Lib2");
        let game2 = synthetic_install(&lib2.join("steamapps").join("common").join("Borderlands 2"));
        let escaped = |p: &Path| p.display().to_string().replace('\\', "\\\\");
        let vdf = |libraries: &[&Path]| {
            let mut text = String::from("\"libraryfolders\"\n{\n");
            for (i, library) in libraries.iter().enumerate() {
                text += &format!(
                    "\t\"{i}\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n",
                    escaped(library)
                );
            }
            text + "}\n"
        };
        let vdf_path = steam.join("steamapps").join("libraryfolders.vdf");
        std::fs::write(&vdf_path, vdf(&[&steam, &lib2])).unwrap();
        assert_eq!(GameInstall::locate_in(&steam).unwrap().root(), game2);

        // A library whose app manifest names another folder.
        let lib3 = tmp.path().join("Lib3");
        let game3 = synthetic_install(&lib3.join("steamapps").join("common").join("BL2 Renamed"));
        std::fs::write(
            lib3.join("steamapps").join("appmanifest_49520.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"49520\"\n\t\"installdir\"\t\t\"BL2 Renamed\"\n}\n",
        )
        .unwrap();
        std::fs::write(&vdf_path, vdf(&[&steam, &lib3, &lib2])).unwrap();
        assert_eq!(GameInstall::locate_in(&steam).unwrap().root(), game3);

        // Nothing installed anywhere.
        let empty = tmp.path().join("Empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(GameInstall::locate_in(&empty), None);

        // Several Steam folders are tried in order; the override replaces the search entirely.
        let roots = [empty.clone(), steam.clone()];
        assert_eq!(
            GameInstall::locate_from(None, &roots).unwrap().root(),
            game3
        );
        assert_eq!(
            GameInstall::locate_from(Some(OsString::new()), &roots)
                .unwrap()
                .root(),
            game3
        );
        let chosen = GameInstall::locate_from(Some(game2.clone().into_os_string()), &roots);
        assert_eq!(chosen.unwrap().root(), game2);
        assert_eq!(
            GameInstall::locate_from(Some(empty.clone().into_os_string()), &roots),
            None
        );
        assert_eq!(GameInstall::locate_from(None, &[]), None);
    }

    #[test]
    fn finds_dlc_packages_and_ids() {
        let tmp = TempDir::new();
        let root = synthetic_install(&tmp.path().join("game"));
        let install = GameInstall::new(&root).unwrap();
        assert!(install.dlc_startup_packages().is_empty());
        assert!(install.dlc_package_ids().is_empty());
        assert_eq!(install.source_packages(), [install.startup_package()]);

        let dlc = |name: &str, package: bool, ini: Option<&str>| {
            let compat = root.join("DLC").join(name).join("Compat");
            std::fs::create_dir_all(compat.join("Content")).unwrap();
            std::fs::create_dir_all(compat.join("Config")).unwrap();
            if package {
                std::fs::write(
                    compat
                        .join("Content")
                        .join(format!("{name}_Startup_SF.upk")),
                    b"x",
                )
                .unwrap();
            }
            if let Some(ini) = ini {
                std::fs::write(compat.join("Config").join("WillowDlc.ini"), ini).unwrap();
            }
        };
        // The ini repeats the id per section; `.PackageId=` lines are not the one wanted.
        dlc("Tulip", true, Some("\r\n[DlcInstalledContent]\r\n.PackageId=7\r\n\r\n[DlcAlwaysLoadedPackages]\r\n+PackageId=2\r\n+Package=GD_TulipPackageDef\r\n\r\n[DlcStartupPackages]\r\n+PackageId=3\r\n"));
        dlc(
            "Orchid",
            true,
            Some("[DlcAlwaysLoadedPackages]\n+PackageId=1\n"),
        );
        dlc("NoPackage", false, Some("+PackageId=40\n"));
        dlc("NoIni", true, None);
        dlc("BadIni", true, Some("+PackageId=many\n"));
        std::fs::create_dir_all(root.join("DLC").join("POCollectors").join("Lic")).unwrap();

        let names: Vec<String> = install
            .dlc_startup_packages()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "BadIni_Startup_SF.upk",
                "NoIni_Startup_SF.upk",
                "Orchid_Startup_SF.upk",
                "Tulip_Startup_SF.upk"
            ]
        );
        let ids = install.dlc_package_ids();
        assert_eq!(
            ids.iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect::<Vec<_>>(),
            [("NoPackage", 40), ("Orchid", 1), ("Tulip", 2)]
        );
        assert_eq!(install.source_packages().len(), 5);
        assert_eq!(install.source_packages()[0], install.startup_package());
    }

    #[test]
    fn package_id_from_utf16_ini() {
        let mut ini = vec![0xFF, 0xFE];
        for unit in "[A]\r\n+PackageId=19\r\n".encode_utf16() {
            ini.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(first_package_id(&ini), Some(19));
        assert_eq!(first_package_id(b"+PackageId= 5 \n+PackageId=6"), Some(5));
        assert_eq!(first_package_id(b"PackageId=5"), None);
    }

    #[test]
    fn fingerprint_changes_with_content_and_packages() {
        let tmp = TempDir::new();
        let root = synthetic_install(&tmp.path().join("game"));
        let install = GameInstall::new(&root).unwrap();
        let first = install.fingerprint().unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(install.fingerprint().unwrap(), first);

        std::fs::write(install.startup_package(), [1, 2, 3]).unwrap();
        let changed = install.fingerprint().unwrap();
        assert_ne!(changed, first);

        // A new DLC startup package changes it too.
        let content = root
            .join("DLC")
            .join("Tulip")
            .join("Compat")
            .join("Content");
        std::fs::create_dir_all(&content).unwrap();
        std::fs::write(content.join("Tulip_Startup_SF.upk"), [9]).unwrap();
        assert_ne!(install.fingerprint().unwrap(), changed);

        let canceled = AtomicBool::new(true);
        assert_eq!(
            install.fingerprint_unless(&canceled),
            Err(GameDataError::Canceled)
        );
    }
}
