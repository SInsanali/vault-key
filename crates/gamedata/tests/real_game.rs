//! Tests against the real, installed game. They only read the install, write only to fresh
//! temporary folders, and skip (pass with a note) when no install is found.
//!
//! The install is found with `GameInstall::locate`: the `GK_GAME_DATA` environment variable if
//! set, otherwise the default Steam folders. Run with `--nocapture` to see the census and
//! timing figures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gamedata::package::{decompress, PropertyList, PropertyValue, UPackage};
use gamedata::{
    build_database, import, load_or_import, AssetCategory, DatabaseCache, DlcResolution,
    GameDataError, GameInstall, GearClass, ImportPhase, ImportProgress, ItemCategory, ItemDatabase,
    ItemInfo, ItemRefs, ObjectInfo, RarityInfo, RarityTier, RarityValue, Rgba, WeaponKind,
    PART_SLOTS,
};

// MARK: Support

/// The install's source packages, parsed once, and the databases built from them.
struct Game {
    install: GameInstall,
    /// `Startup.upk`, then the DLC startup packages in name order.
    packages: Vec<UPackage>,
    /// Built from every package.
    db: ItemDatabase,
    /// Built from `Startup.upk` alone, as the Swift version did.
    base_db: ItemDatabase,
}

fn load() -> Option<Game> {
    let install = GameInstall::locate()?;
    let packages: Vec<UPackage> = install
        .source_packages()
        .iter()
        .map(|path| UPackage::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect();
    let db = build_database(&packages, "test").expect("database from all packages");
    let base_db = build_database(&packages[..1], "test").expect("database from Startup.upk");
    Some(Game {
        install,
        packages,
        db,
        base_db,
    })
}

fn game() -> Option<&'static Game> {
    static GAME: OnceLock<Option<Game>> = OnceLock::new();
    let game = GAME.get_or_init(load).as_ref();
    if game.is_none() {
        eprintln!("skipped: no Borderlands 2 install found (set GK_GAME_DATA to point at one)");
    }
    game
}

/// A fresh folder under the system temp directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "gk-gamedata-real-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn startup(g: &Game) -> &UPackage {
    &g.packages[0]
}

fn props(pkg: &UPackage, path: &str) -> PropertyList {
    let index = pkg
        .export_index(path)
        .unwrap_or_else(|| panic!("missing {path}"));
    pkg.properties_of_export(index).unwrap()
}

fn hex(color: Option<Rgba>) -> Option<String> {
    color.map(Rgba::hex)
}

/// The value of `path` in the base game's tables.
fn base_value(db: &ItemDatabase, path: &str, category: AssetCategory) -> u32 {
    db.value_of(path, category, 0)
        .unwrap_or_else(|| panic!("{path} is not in the base tables"))
}

/// The value an item of `set` stores for `path` in that set's own tables.
fn dlc_value(db: &ItemDatabase, path: &str, category: AssetCategory, set: u32) -> u32 {
    db.value_of(path, category, set)
        .unwrap_or_else(|| panic!("{path} is not in set {set}"))
}

// MARK: Known items

/// An item whose in-game appearance is known. The numbers are its decoded bitstream fields
/// (format doc §6.6); `code` is the item code they were decoded from. Rows 5–16 are items on
/// the owner's main character. Levels follow the summing rarity rule (spike doc §5).
struct Known {
    #[allow(dead_code)]
    code: &'static str,
    refs: ItemRefs,
    name: Option<&'static str>,
    level: i64,
    tier: RarityTier,
    hex: &'static str,
    manufacturer: Option<&'static str>,
    category: ItemCategory,
    gear: GearClass,
}

fn weapon(item_type: u32, balance: u32, manufacturer: u32, parts: [u32; PART_SLOTS]) -> ItemRefs {
    ItemRefs {
        is_weapon: true,
        set: Some(0),
        item_type: Some(item_type),
        balance: Some(balance),
        manufacturer: Some(manufacturer),
        parts: parts.map(Some),
    }
}

/// An item whose bitstream holds the first `parts.len()` part slots.
fn item(item_type: u32, balance: u32, manufacturer: u32, parts: &[u32]) -> ItemRefs {
    let mut slots = [None; PART_SLOTS];
    for (slot, &value) in slots.iter_mut().zip(parts) {
        *slot = Some(value);
    }
    ItemRefs {
        is_weapon: false,
        set: Some(0),
        item_type: Some(item_type),
        balance: Some(balance),
        manufacturer: Some(manufacturer),
        parts: slots,
    }
}

const W: u32 = 131071;
const I: u32 = 65535;

fn known_items() -> Vec<Known> {
    use GearClass::{Gear, NotGear};
    use ItemCategory::{Customization, GrenadeMod, Relic, Shield, Usable, Weapon};
    use RarityTier::{Common, Legendary, Rare, Uncommon};
    use WeaponKind::{Pistol, Shotgun, SniperRifle};
    let row = |code, refs, name, level, tier, hex, manufacturer, category, gear| Known {
        code,
        refs,
        name,
        level,
        tier,
        hex,
        manufacturer,
        category,
        gear,
    };
    vec![
        // 1: the reference README's weapon.
        row(
            "BL2(h0Hd1Z+jY/s2Qy++Zu8Ba9qXoOmjwJ6NhrlsOmhNMX+oJo5CfQns)",
            weapon(
                1924,
                131111,
                133,
                [
                    12538, 12466, 12552, 12301, W, 12306, 12313, W, 12554, 12352, 12568,
                ],
            ),
            Some("Righteous Infinity"),
            9,
            Legendary,
            "#FFB400",
            Some("Vladof"),
            Weapon(Pistol),
            Gear,
        ),
        // 2: the reference README's item.
        row(
            "BL2(B2vuv4tz1zSQCf2pqLJCS5XD/tKN4FXpjRJLnn1v85U=)",
            item(2840, 27695, 137, &[0, 1, I, I, I, I, 52, 14]),
            Some("Vault Hunter's Relic"),
            3,
            Rare,
            "#3C8EFF",
            Some("Eridian"),
            Relic,
            Gear,
        ),
        // 3: a new character's Basic Repeater.
        row(
            "BL2(hwAAAAC8CQCBxwJABBEI0MJghdENIwHG//////////97w/iGMQzj)",
            weapon(
                1921,
                131094,
                130,
                [12468, 12460, 12509, 12297, W, W, W, W, 12510, 12511, 12483],
            ),
            Some("Basic Repeater"),
            1,
            Common,
            "#FFFFFF",
            Some("Dahl"),
            Weapon(Pistol),
            Gear,
        ),
        // 4: the intro ECHO (BD_AssassinIntroEcho). Its colour row is white with alpha 0.
        row(
            "BL2(BwAAAABsWgADYgaQ4/8AwA==)",
            item(25091, 116739, 2047, &[]),
            Some("Helmet Data Rec0rding"),
            502,
            RarityTier::Unknown,
            "#FFFFFF",
            None,
            Usable,
            NotGear,
        ),
        // 5: a Siren skin. Its bitstream has no part slots.
        row(
            "BL2(BwAAAAB9ugBNBJog4P+DwQ==)",
            item(1101, 4173, 2047, &[]),
            None,
            2,
            Uncommon,
            "#3DD20B",
            None,
            Customization,
            NotGear,
        ),
        row(
            "BL2(BwAAAABBUAAIERSg4xCFggAEBEQGRCPE/////////38WBEUEFsQ=)",
            item(
                4360,
                118794,
                135,
                &[4098, 4112, 4121, 4237, I, I, I, I, 4185, 4372, 4184],
            ),
            Some("Quick Shield"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Tediore"),
            Shield,
            Gear,
        ),
        row(
            "BL2(BwAAAAA/RwAIETag4xCFAgLEBIQGRCPE/////////78iREMEFsQ=)",
            item(
                4360,
                118811,
                135,
                &[4104, 4115, 4122, 4237, I, I, I, I, 4234, 4365, 4184],
            ),
            Some("Cut Rate Shield"),
            1,
            Common,
            "#FFFFFF",
            Some("Tediore"),
            Shield,
            Gear,
        ),
        row(
            "BL2(BwAAAAD7EQABDwSYIxAEwhlDAEMBgwFDDUMGw////78RAwmDLMM=)",
            item(
                3841,
                117762,
                129,
                &[3175, 3073, 3077, 3078, 3125, 3097, I, I, 3142, 3108, 3250],
            ),
            Some("Throwin gurnade"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Bandit Made"),
            GrenadeMod,
            Gear,
        ),
        row(
            "BL2(hwAAAACU0wCAZwZAAkEggMNYhQEUIwHG//8JGBcw/v/rxJCJMRTj)",
            weapon(
                1920,
                131123,
                129,
                [
                    12512, 12459, 12608, 12297, W, 12307, 12311, W, 12602, 12594, 12611,
                ],
            ),
            Some("Flynt's Tinderbox"),
            3,
            Rare,
            "#3C8EFF",
            Some("Bandit Made"),
            Weapon(Pistol),
            Gear,
        ),
        row(
            "BL2(hwAAAAB5GACEiIFBBlEooIFZACMB5gWMBZj//2Rg/v+ngXEEowHm)",
            weapon(
                2180,
                134156,
                131,
                [
                    24680, 24587, 24594, 24623, 24598, W, 24676, W, 24681, 24718, 24602,
                ],
            ),
            Some("Skookum Diaub"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Jakobs"),
            Weapon(SniperRifle),
            Gear,
        ),
        row(
            "BL2(hwAAAAAz+wCBRwBABEEgjMB4hREAIwHG/38JGBow/v+TwBiCMQzj)",
            weapon(
                1921,
                131074,
                130,
                [
                    12323, 12463, 12289, 12297, W, 12306, 12314, W, 12324, 12355, 12483,
                ],
            ),
            Some("Twin Repeater"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Dahl"),
            Weapon(Pistol),
            Gear,
        ),
        row(
            "BL2(hwAAAADyugCEZwBAClEotMCIhTEAIwHG/38JGBcw/v+7wKBA0gjj)",
            weapon(
                1924,
                131075,
                133,
                [
                    12333, 12465, 12291, 12297, W, 12306, 12311, W, 12334, 18452, 12429,
                ],
            ),
            Some("Oppressed Anarchist"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Vladof"),
            Weapon(Pistol),
            Gear,
        ),
        row(
            "BL2(hwAAAAAFAgCGpwBAEFEonMB4hWEAIwHG//8JGBcw/v+jwMiEYQzj)",
            weapon(
                1926,
                131077,
                136,
                [
                    12327, 12463, 12294, 12297, W, 12307, 12311, W, 12328, 12441, 12486,
                ],
            ),
            Some("Inflammatory Aegis"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Maliwan"),
            Weapon(Pistol),
            Gear,
        ),
        row(
            "BL2(hwAAAACRGwCGpwBAEFEonMBwhTEAIwHG//8JGBcw/v+jwMiEoQjj)",
            weapon(
                1926,
                131077,
                136,
                [
                    12327, 12462, 12291, 12297, W, 12307, 12311, W, 12328, 12441, 12426,
                ],
            ),
            Some("Inflammatory Umbrage"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Maliwan"),
            Weapon(Pistol),
            Gear,
        ),
        row(
            "BL2(hwAAAACUbgAD6IFADlEoSEJRg0IApQBKG5T///////9PQpFAIgXl)",
            weapon(
                2051,
                132111,
                135,
                [
                    20626, 20586, 20484, 20485, 20589, W, W, W, 20627, 18450, 20562,
                ],
            ),
            Some("Bulk Double Barrels!"),
            1,
            Common,
            "#FFFFFF",
            Some("Tediore"),
            Weapon(Shotgun),
            Gear,
        ),
        row(
            "BL2(hwAAAADOrACABwBAAlEogMB4hVEAAwLG/38JGB0w/v+LwOiC4Qfj)",
            weapon(
                1920,
                131072,
                129,
                [
                    12320, 12463, 12293, 12304, W, 12306, 12317, W, 12322, 12381, 12414,
                ],
            ),
            Some("murduerer's Magamum!"),
            2,
            Uncommon,
            "#3DD20B",
            Some("Bandit Made"),
            Weapon(Pistol),
            Gear,
        ),
    ]
}

/// Thunderball Fists, built from row 1 with every value looked up in `db`.
fn thunderball_fists(db: &ItemDatabase) -> ItemRefs {
    use AssetCategory::{Balances, Manufacturers, WeaponParts, WeaponTypes};
    let part = |path| Some(base_value(db, path, WeaponParts));
    ItemRefs {
        is_weapon: true,
        set: Some(0),
        item_type: Some(base_value(
            db,
            "GD_Weap_Pistol.A_Weapons.WeaponType_Maliwan_Pistol",
            WeaponTypes,
        )),
        balance: Some(base_value(
            db,
            "GD_Weap_Pistol.A_Weapons_Legendary.Pistol_Maliwan_5_ThunderballFists",
            Balances,
        )),
        manufacturer: Some(base_value(
            db,
            "GD_Manufacturers.Manufacturers.Maliwan",
            Manufacturers,
        )),
        parts: [
            part("GD_Weap_Pistol.Body.Pistol_Body_Maliwan_4"),
            Some(W), // grip
            part("GD_Weap_Pistol.Barrel.Pistol_Barrel_Maliwan_ThunderballFists"),
            Some(W), // sight
            Some(W), // stock
            part("GD_Weap_Pistol.elemental.Pistol_Elemental_Shock"),
            Some(W), // accessory 1
            Some(W), // accessory 2
            part("GD_Weap_Pistol.ManufacturerMaterials.Mat_Maliwan_5_Legendary"),
            Some(W), // prefix
            part("GD_Weap_Pistol.Name.Title_Maliwan.Title_Legendary_ThunderballFists"),
        ],
    }
}

// MARK: Package files

#[test]
fn startup_matches_its_sidecar() {
    let Some(g) = game() else { return };
    let path = g.install.startup_package();
    let raw = std::fs::read(&path).unwrap();
    // Whole-file compressed: where a package has its version, this has the block size's low half.
    assert_eq!(
        u32::from_le_bytes(raw[0..4].try_into().unwrap()),
        0x9E2A_83C1
    );
    assert_eq!(
        u32::from_le_bytes(raw[4..8].try_into().unwrap()),
        0x0002_0000
    );
    let out = decompress(&raw, "Startup.upk").unwrap();
    let sidecar = std::fs::read_to_string(path.with_extension("upk.uncompressed_size")).unwrap();
    assert_eq!(sidecar.trim().parse::<usize>().unwrap(), out.len());
    assert_eq!(out.len(), startup(g).bytes().len());
    println!(
        "Startup.upk: {} bytes compressed, {} decompressed",
        raw.len(),
        out.len()
    );
}

#[test]
fn startup_tables() {
    let Some(g) = game() else { return };
    let pkg = startup(g);
    assert_eq!((pkg.version, pkg.licensee), (832, 46));
    assert_eq!(pkg.names.len(), 28_576);
    assert_eq!(pkg.imports.len(), 760);
    assert_eq!(pkg.exports.len(), 55_087);
    assert_eq!(pkg.exports_of_class("WeaponNamePartDefinition").len(), 748);
    assert_eq!(pkg.exports_of_class("ItemNamePartDefinition").len(), 498);
    let globals = pkg.export_index("GD_Globals.General.Globals").unwrap();
    assert_eq!(pkg.class_name_of_export(globals), "GlobalsDefinition");
}

#[test]
fn dlc_startup_packages_use_the_chunk_table_layout() {
    let Some(g) = game() else { return };
    let paths = g.install.dlc_startup_packages();
    assert_eq!(paths.len() + 1, g.packages.len());
    for (path, pkg) in paths.iter().zip(&g.packages[1..]) {
        let raw = std::fs::read(path).unwrap();
        // A normal header: version 832, licensee 46, straight after the tag.
        assert_eq!(
            raw[..8],
            [0xC1, 0x83, 0x2A, 0x9E, 0x40, 0x03, 0x2E, 0x00],
            "{}",
            pkg.name
        );
        assert!(pkg.bytes().len() > raw.len(), "{} is compressed", pkg.name);
        assert_eq!((pkg.version, pkg.licensee), (832, 46));
        assert!(!pkg.names.is_empty() && !pkg.imports.is_empty() && !pkg.exports.is_empty());
        // Every export has a path and can be found by it.
        for index in 0..pkg.exports.len() {
            let path = pkg.export_path(index).unwrap();
            assert!(pkg.export_index(&path).is_some(), "{}: {path}", pkg.name);
        }
    }
}

#[test]
fn real_definitions() {
    let Some(g) = game() else { return };
    let pkg = startup(g);

    let part = props(
        pkg,
        "GD_Weap_Pistol.Name.Title_Vladof.Title_Legendary_Infinity",
    );
    assert_eq!(part.string("PartName"), Some("Infinity"));
    assert_eq!(part.float("Priority"), Some(10.0));

    let vladof = props(pkg, "GD_Manufacturers.Manufacturers.Vladof");
    let grades = pkg.struct_array(vladof.get("Grades").unwrap()).unwrap();
    assert_eq!(grades[0].string("DisplayName"), Some("Vladof"));

    let globals = props(pkg, "GD_Globals.General.Globals");
    let colors = pkg
        .struct_array(globals.get("RarityLevelColors").unwrap())
        .unwrap();
    assert_eq!(colors.len(), 18);
    assert_eq!(
        colors[2].get("Color"),
        Some(&PropertyValue::StructRaw {
            name: "Color".into(),
            bytes: vec![0x0B, 0xD2, 0x3D, 0xFF]
        })
    );
    assert_eq!(colors[2].enum_name("RarityRating"), Some("RARITY_Uncommon"));
    let library = globals.object_at("AssetLibraries", 5).unwrap();
    assert_eq!(
        pkg.path_of(library).unwrap(),
        "GD_AssetLibraries.Inventory.AL_Balance"
    );

    let echo = props(pkg, "GD_DefaultProfiles.IntroEchos.ID_AssassinIntroECHO");
    assert_eq!(echo.string("ItemName"), Some("Helmet Data Rec0rding"));
    assert_eq!(echo.bool("bItemNameIsFullName"), Some(true));
    assert_eq!(
        echo.struct_fields("BaseRarity")
            .and_then(|r| r.float("BaseValueConstant")),
        Some(502.0)
    );

    let pistol = props(pkg, "GD_Weap_Pistol.A_Weapons.WeaponType_Vladof_Pistol");
    assert_eq!(pistol.get("WeaponType"), None);
    assert_eq!(pistol.string("Typename"), Some("Pistol"));
    let sniper = props(
        pkg,
        "GD_Weap_SniperRifles.A_Weapons.WeaponType_Jakobs_Sniper",
    );
    assert_eq!(sniper.enum_name("WeaponType"), Some("WT_SniperRifle"));
}

// MARK: Base-game database

#[test]
fn readme_vectors_resolve() {
    let Some(g) = game() else { return };
    use AssetCategory::{Balances, ItemTypes, Manufacturers, WeaponTypes};
    let vectors = [
        (
            Balances,
            131111,
            "GD_Weap_Pistol.A_Weapons_Legendary.Pistol_Vladof_5_Infinity",
        ),
        (
            WeaponTypes,
            1924,
            "GD_Weap_Pistol.A_Weapons.WeaponType_Vladof_Pistol",
        ),
        (Manufacturers, 133, "GD_Manufacturers.Manufacturers.Vladof"),
        (Balances, 27695, "GD_Artifacts.A_Item_Unique.A_VaultHunter"),
        (
            ItemTypes,
            2840,
            "GD_Artifacts.A_Item_Unique.Artifact_VaultHunter",
        ),
        (
            Manufacturers,
            137,
            "GD_Manufacturers.Artifacts.Artifact_TypeA",
        ),
    ];
    for db in [&g.base_db, &g.db] {
        for (category, value, path) in vectors {
            assert_eq!(
                db.path(category, value, 0),
                Some(path),
                "{category:?} {value}"
            );
            assert_eq!(db.value_of(path, category, 0), Some(value), "{path}");
        }
        assert_eq!(
            db.object(vectors[1].2),
            Some(&ObjectInfo::WeaponType {
                kind: Some(WeaponKind::Pistol),
                type_name: Some("Pistol".into()),
                base_rarity: RarityValue::None
            })
        );
        assert_eq!(
            db.object(vectors[2].2),
            Some(&ObjectInfo::Manufacturer {
                display_name: Some("Vladof".into())
            })
        );
        assert_eq!(
            db.object("GD_Weap_SniperRifles.A_Weapons.WeaponType_Jakobs_Sniper"),
            Some(&ObjectInfo::WeaponType {
                kind: Some(WeaponKind::SniperRifle),
                type_name: Some("Sniper Rifle".into()),
                base_rarity: RarityValue::None
            })
        );
        assert!(matches!(
            db.object(vectors[0].2),
            Some(ObjectInfo::Balance { .. })
        ));
    }
}

/// Per category: libs, asset entries, objects found in the package, unresolved sublibraries.
/// These were measured on the Mac build and are the same on the Windows build.
#[test]
fn base_library_shapes() {
    let Some(g) = game() else { return };
    use AssetCategory::*;
    let expected = [
        (WeaponTypes, 39, 75, 33, 5),
        (WeaponParts, 14, 1508, 1406, 2),
        (ItemTypes, 103, 916, 698, 1),
        (ItemParts, 6, 930, 857, 0),
        (Manufacturers, 2, 18, 18, 0),
        (Balances, 141, 1373, 1083, 6),
    ];
    for db in [&g.base_db, &g.db] {
        let set = &db.sets[&0];
        assert_eq!(set.dlc_name, None);
        for (category, libs, entries, located, unresolved) in expected {
            let tables = set.tables(category);
            assert_eq!(tables.len(), libs, "{category:?} libs");
            assert_eq!(
                tables.iter().map(Vec::len).sum::<usize>(),
                entries,
                "{category:?} entries"
            );
            let found = set
                .paths(category)
                .filter(|p| db.object(p).is_some())
                .count();
            assert_eq!(found, located, "{category:?} located");
            assert_eq!(
                set.unresolved_sublibraries(category).len(),
                unresolved,
                "{category:?} unresolved"
            );
            // Half of each category's lib range is left for DLC sets (the flag bit).
            assert!(libs < category.dlc_flag() as usize, "{category:?}");
        }
        assert!(set
            .unresolved_sublibraries(ItemTypes)
            .contains(&"GD_Z1_CutToTheChaseData.PackageAssetSublibrary_109".to_string()));
    }
    assert_eq!(g.base_db.sets.len(), 1);
    assert!(g.base_db.skipped_packages.is_empty());
}

#[test]
fn rarity_values_and_colours() {
    let Some(g) = game() else { return };
    let db = &g.base_db;
    let rarity = |path: &str| db.object(path).and_then(ObjectInfo::rarity);
    assert_eq!(
        rarity("GD_Weap_Pistol.Barrel.Pistol_Barrel_Vladof_Infinity"),
        Some(RarityValue::Value(5.0))
    );
    assert_eq!(
        rarity("GD_Weap_Pistol.Body.Pistol_Body_Vladof_4"),
        Some(RarityValue::Value(4.0))
    );
    assert_eq!(
        rarity("GD_Weap_Pistol.Grip.Pistol_Grip_Hyperion"),
        Some(RarityValue::None)
    );
    assert_eq!(
        db.object("GD_DefaultProfiles.IntroEchos.ID_AssassinIntroECHO"),
        Some(&ObjectInfo::ItemType {
            class_name: "UsableItemDefinition".into(),
            item_name: Some("Helmet Data Rec0rding".into()),
            name_is_full: true,
            base_rarity: RarityValue::Value(502.0)
        })
    );

    assert_eq!(db.rarity_colors.len(), 18);
    for (level, expected) in [
        (1, "#FFFFFF"),
        (2, "#3DD20B"),
        (3, "#3C8EFF"),
        (4, "#A83FE5"),
        (5, "#FFB400"),
        (6, "#CA00A8"),
        (9, "#FFB400"),
        (500, "#00FFFF"),
        (501, "#FF9AB8"),
        (506, "#F2FFA1"),
    ] {
        assert_eq!(
            db.color_row(level).map(|r| r.color.hex()).as_deref(),
            Some(expected),
            "level {level}"
        );
    }
    let nine = db.color_row(9).unwrap();
    assert_eq!(
        (nine.rating.as_str(), nine.min_level, nine.max_level),
        ("RARITY_Legendary", 7, 10)
    );
    assert_eq!(db.color_row(501).unwrap().rating, "RARITY_Seraph");
    assert_eq!(db.color_row(500).unwrap().rating, "RARITY_Unknown");
    assert_eq!(db.color_row(506).unwrap().rating, "RARITY_Rainbow");
    assert_eq!(db.color_row(11), None);
    assert_eq!(db.color_row(505), None);
    assert_eq!(g.db.rarity_colors, db.rarity_colors);
}

/// Ledger rulings (b) and (c) against the game's own colour table.
#[test]
fn game_colour_table_edges() {
    let Some(g) = game() else { return };
    let db = &g.base_db;
    let at = |level| db.rarity_for_level(Some(level));
    assert_eq!(
        at(11),
        RarityInfo {
            level: Some(11),
            tier: RarityTier::Unknown,
            color: None
        }
    );
    assert_eq!(db.color_row(12).unwrap().color, Rgba::new(0, 0, 0, 0));
    assert_eq!(at(12).color, None);
    assert_eq!(at(170).color, None);
    // Seraph's row has alpha 0 but is shown pink in game: known, alpha as stored.
    assert_eq!(at(501).tier, RarityTier::Seraph);
    assert_eq!(at(501).color, Some(Rgba::new(0xFF, 0x9A, 0xB8, 0)));
    // The intro ECHO's row (502) is white with alpha 0.
    assert_eq!(at(502).color, Some(Rgba::new(255, 255, 255, 0)));
    assert_eq!(db.color_row(6).unwrap().rating, "RARITY_VeryRare");
    assert_eq!(at(6).tier, RarityTier::ETech);
    assert_eq!(hex(at(6).color).as_deref(), Some("#CA00A8"));
    assert_eq!(at(0).color, Some(Rgba::new(0xCD, 0xC1, 0xAF, 0)));
    assert_eq!(at(5).tier, RarityTier::Legendary);
    assert_eq!(
        at(500),
        RarityInfo {
            level: Some(500),
            tier: RarityTier::Pearlescent,
            color: Some(Rgba::new(0, 255, 255, 0))
        }
    );
    assert_eq!(at(506).tier, RarityTier::Rainbow);
    assert_eq!(db.rarity_for_level(None), RarityInfo::UNKNOWN);
}

/// The path of a part's `Rarity.BaseValueAttribute`, if it has one.
fn rarity_attribute_of(pkg: &UPackage, path: &str) -> Option<String> {
    let index = pkg.export_index(path)?;
    let list = pkg.properties_of_export(index).ok()?;
    let reference = list
        .struct_fields("Rarity")?
        .object("BaseValueAttribute")
        .filter(|&r| r != 0)?;
    pkg.path_of(reference).ok()
}

/// Spike doc §5: the evidence that an item's rarity level is a sum.
#[test]
fn base_rarity_census() {
    let Some(g) = game() else { return };
    let db = &g.base_db;
    const ALIEN: &str = "GD_Balance_Inventory.Rarity_Item.ItemRarity4_VeryRare_Alien";
    let mut values = BTreeSet::new();
    let mut unresolvable = Vec::new();
    let mut alien_parts = Vec::new();
    for (path, info) in &db.objects {
        if matches!(info, ObjectInfo::Part { .. })
            && rarity_attribute_of(startup(g), path).as_deref() == Some(ALIEN)
        {
            alien_parts.push(path.as_str());
        }
        match info.rarity() {
            Some(RarityValue::Value(v)) => {
                assert_eq!(v.fract(), 0.0, "{path}");
                values.insert(v as i64);
            }
            Some(RarityValue::Unresolvable) => unresolvable.push(path.as_str()),
            Some(RarityValue::None) | None => {}
        }
    }
    println!("base rarity values: {values:?}");
    // (a) Every value is one of the known levels; (b) none lies in the e-tech or legendary rows
    // 6…10, which only a sum can reach.
    assert_eq!(
        values,
        BTreeSet::from([1, 2, 3, 4, 5, 171, 176, 181, 502, 503, 504])
    );
    // (c) Every rarity struct on a part or type resolves statically.
    assert!(unresolvable.is_empty(), "{unresolvable:?}");
    // (d) Parts using the alien rarity (value 2) are e-tech barrels by path.
    assert_eq!(alien_parts.len(), 15);
    for path in alien_parts {
        assert!(path.contains("Alien") || path.contains("ETech"), "{path}");
        assert_eq!(
            db.object(path).and_then(ObjectInfo::rarity),
            Some(RarityValue::Value(2.0)),
            "{path}"
        );
    }
}

// MARK: Item info

#[test]
fn known_items_match_expected() {
    let Some(g) = game() else { return };
    for db in [&g.base_db, &g.db] {
        for (i, row) in known_items().iter().enumerate() {
            let n = i + 1;
            let info = db.info(&row.refs);
            assert_eq!(info.name.as_deref(), row.name, "row {n} name");
            assert_eq!(info.rarity.level, Some(row.level), "row {n} level");
            assert_eq!(info.rarity.tier, row.tier, "row {n} tier");
            assert_eq!(
                hex(info.rarity.color).as_deref(),
                Some(row.hex),
                "row {n} colour"
            );
            assert_eq!(
                info.manufacturer.as_deref(),
                row.manufacturer,
                "row {n} manufacturer"
            );
            assert_eq!(info.category, row.category, "row {n} category");
            assert_eq!(info.gear_class, row.gear, "row {n} gear");
            assert_eq!(db.gear_class(&row.refs), row.gear, "row {n} gear_class()");
            // Base-game items never set the bit that marks a DLC reference.
            assert_eq!(
                db.info_with(&row.refs, DlcResolution::Verified),
                info,
                "row {n}"
            );
        }
        // Row 4, the intro ECHO: alpha kept as stored.
        let echo = db.info(&known_items()[3].refs);
        assert_eq!(echo.rarity.color, Some(Rgba::new(255, 255, 255, 0)));
    }
}

/// No value in a known base-game item has its category's top lib bit set (the all-ones "empty"
/// values aside), which is what leaves that bit free to mark DLC references.
#[test]
fn base_item_values_never_use_the_dlc_flag() {
    for (i, row) in known_items().iter().enumerate() {
        let refs = &row.refs;
        let check = |category: AssetCategory, value: Option<u32>, what: &str| {
            let Some(value) = value else { return };
            if value == category.all_ones() {
                return;
            }
            let (lib, _) = category.split(value);
            assert_eq!(lib & category.dlc_flag(), 0, "row {} {what} {value}", i + 1);
        };
        check(AssetCategory::types(refs.is_weapon), refs.item_type, "type");
        check(AssetCategory::Balances, refs.balance, "balance");
        check(
            AssetCategory::Manufacturers,
            refs.manufacturer,
            "manufacturer",
        );
        for part in refs.parts {
            check(AssetCategory::parts(refs.is_weapon), part, "part");
        }
    }
}

#[test]
fn thunderball_fists_is_legendary() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let refs = thunderball_fists(db);
    // Type, body, barrel, elemental, material, title.
    use RarityValue::{None as Absent, Value};
    assert_eq!(
        db.rarity_contributions(&refs, DlcResolution::Unverified),
        [Absent, Value(4.0), Value(5.0), Absent, Absent, Absent]
    );
    let info = db.info(&refs);
    assert_eq!(info.name.as_deref(), Some("Thunderball Fists"));
    assert_eq!(info.rarity.level, Some(9));
    assert_eq!(info.rarity.tier, RarityTier::Legendary);
    assert_eq!(hex(info.rarity.color).as_deref(), Some("#FFB400"));
    assert_eq!(info.manufacturer.as_deref(), Some("Maliwan"));
    assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));
    assert_eq!(info.gear_class, GearClass::Gear);
}

#[test]
fn unresolvable_references_yield_unknown() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let base = known_items().swap_remove(0).refs;
    let out_of_range = (63 << 11) | 5;
    let pistol = ItemCategory::Weapon(WeaponKind::Pistol);

    let mut refs = base.clone();
    refs.parts[10] = Some(out_of_range);
    let info = db.info(&refs);
    assert_eq!(info.name, None);
    assert_eq!(info.category, pistol);
    assert_eq!(info.manufacturer.as_deref(), Some("Vladof"));
    assert_eq!(info.gear_class, GearClass::Gear);

    // A title slot holding a part that is not a name part.
    let mut refs = base.clone();
    refs.parts[10] = Some(base_value(
        db,
        "GD_Weap_Pistol.Body.Pistol_Body_Vladof_4",
        AssetCategory::WeaponParts,
    ));
    assert_eq!(db.info(&refs).name, None);

    let mut refs = base.clone();
    refs.parts[2] = Some(out_of_range); // barrel
    let info = db.info(&refs);
    assert_eq!(info.rarity, RarityInfo::UNKNOWN);
    assert_eq!(info.name.as_deref(), Some("Righteous Infinity"));

    let mut refs = base.clone();
    refs.item_type = Some((127 << 6) | 63);
    let info = db.info(&refs);
    assert_eq!(
        (info.category, info.gear_class),
        (ItemCategory::Unknown, GearClass::Unknown)
    );

    let mut refs = base.clone();
    refs.set = Some(1);
    assert_eq!(db.info(&refs), ItemInfo::UNKNOWN);
    refs.set = None;
    assert_eq!(db.info(&refs), ItemInfo::UNKNOWN);

    let mut refs = base;
    refs.manufacturer = Some(2047);
    assert_eq!(db.info(&refs).manufacturer, None);
}

#[test]
fn database_classifies_known_items() {
    let Some(g) = game() else { return };
    let rows = known_items();
    assert_eq!(g.db.gear_class(&rows[3].refs), GearClass::NotGear); // intro ECHO
    assert_eq!(g.db.gear_class(&rows[2].refs), GearClass::Gear); // Basic Repeater
    assert_eq!(g.db.gear_class(&rows[4].refs), GearClass::NotGear); // skin
                                                                    // A mission weapon: every base balance of that class makes its weapon not gear.
    let mission: Vec<&str> = g.db.sets[&0]
        .paths(AssetCategory::Balances)
        .filter(|p| {
            matches!(g.db.object(p), Some(ObjectInfo::Balance { class_name }) if class_name == "MissionWeaponBalanceDefinition")
        })
        .collect();
    assert_eq!(mission.len(), 7);
    for path in mission {
        let mut refs = rows[2].refs.clone();
        refs.balance = Some(base_value(&g.db, path, AssetCategory::Balances));
        assert_eq!(g.db.gear_class(&refs), GearClass::NotGear, "{path}");
    }
}

// MARK: DLC sets

/// DLC folder → `PackageId`, as read from each `WillowDlc.ini` on the Windows build. The same
/// table as the Mac build's.
const DLC_PACKAGE_IDS: [(&str, u32); 21] = [
    ("Allium", 19),
    ("Anecana", 24),
    ("Anemone", 22),
    ("Aster", 9),
    ("CommDay2013", 18),
    ("Flax", 5),
    ("Gardenia01", 7),
    ("Gardenia04", 16),
    ("Gardenia05", 17),
    ("Gladiolus", 8),
    ("Iris", 3),
    ("Lilac", 6),
    ("Lobelia", 10),
    ("LootChest2", 14),
    ("Mancana", 23),
    ("Nasturtium", 20),
    ("Orchid", 1),
    ("PN2", 12),
    ("Peony", 21),
    ("Sage", 4),
    ("Tulip", 2),
];

/// The DLCs whose startup package holds an asset-library set.
const DLC_SETS: [(u32, &str); 13] = [
    (1, "Orchid"),
    (2, "Tulip"),
    (3, "Iris"),
    (4, "Sage"),
    (5, "Flax"),
    (6, "Lilac"),
    (8, "Gladiolus"),
    (9, "Aster"),
    (10, "Lobelia"),
    (19, "Allium"),
    (20, "Nasturtium"),
    (21, "Peony"),
    (22, "Anemone"),
];

#[test]
fn dlc_sets_discovered() {
    let Some(g) = game() else { return };
    let packages = g.install.dlc_startup_packages();
    assert_eq!(packages.len(), 21);
    let ids = g.install.dlc_package_ids();
    assert_eq!(
        ids,
        DLC_PACKAGE_IDS
            .iter()
            .map(|&(name, id)| (name.to_string(), id))
            .collect::<BTreeMap<_, _>>()
    );
    // Every folder with an id has a startup package, and the other way round.
    let with_package: BTreeSet<String> = packages
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches("_Startup_SF.upk")
                .to_string()
        })
        .collect();
    assert_eq!(with_package, ids.keys().cloned().collect());

    let db = &g.db;
    assert!(db.skipped_packages.is_empty(), "{:?}", db.skipped_packages);
    let found: Vec<(u32, &str)> = db
        .sets
        .iter()
        .filter(|(id, _)| **id != 0)
        .map(|(id, set)| (*id, set.dlc_name.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(found, DLC_SETS);
    for (id, set) in db.sets.iter().filter(|(id, _)| **id != 0) {
        // The id inside the package (`PackageDef.PackageId`) is the id in that DLC's ini.
        let name = set.dlc_name.as_deref().unwrap();
        assert_eq!(ids.get(name), Some(id), "{name}");
        // Every DLC library fits below the flag bit, and every table within the asset range.
        for category in AssetCategory::ALL {
            assert!(
                set.tables(category).len() < category.dlc_flag() as usize,
                "{name} {category:?}"
            );
            for table in set.tables(category) {
                assert!(
                    table.len() <= 1 << category.asset_bits(),
                    "{name} {category:?}"
                );
            }
        }
    }
    assert_eq!(db.sets[&1].tables(AssetCategory::Balances).len(), 36);

    // Coverage, per set: asset entries and how many are definitions in the startup packages.
    for (id, set) in &db.sets {
        let mut line = format!(
            "set {id:>2} {:<10}",
            set.dlc_name.as_deref().unwrap_or("base")
        );
        for category in AssetCategory::ALL {
            let entries: usize = set.tables(category).iter().map(Vec::len).sum();
            let located = set
                .paths(category)
                .filter(|p| db.object(p).is_some())
                .count();
            line += &format!(" {}/{}/{}", set.tables(category).len(), entries, located);
        }
        println!("{line}   (libs/entries/located per category)");
    }
}

/// When two packages define the same table object, the first one is used. On this install every
/// such pair is the same definition, so the choice does not matter.
#[test]
fn shared_definitions_agree_across_packages() {
    let Some(g) = game() else { return };
    let mut shared = 0;
    for set in g.db.sets.values() {
        for category in AssetCategory::ALL {
            for path in set.paths(category) {
                let mut definitions = g
                    .packages
                    .iter()
                    .filter_map(|pkg| Some((pkg, pkg.export_index(path)?)))
                    .map(|(pkg, index)| (pkg.class_name_of_export(index), scalars(pkg, index)));
                let Some(first) = definitions.next() else {
                    continue;
                };
                for other in definitions {
                    shared += 1;
                    assert_eq!(other, first, "{path}");
                }
            }
        }
    }
    println!("table objects defined again by a later package: {shared}, all identical");
    assert!(shared > 0);
}

/// An export's properties that do not depend on the package's own numbering.
fn scalars(pkg: &UPackage, index: usize) -> Vec<String> {
    let list = pkg.properties_of_export(index).unwrap();
    list.items
        .iter()
        .map(|p| match &p.value {
            PropertyValue::Object(_) => format!("{}[{}] object", p.name, p.array_index),
            PropertyValue::Array { count, .. } => {
                format!("{}[{}] array of {count}", p.name, p.array_index)
            }
            PropertyValue::Struct(fields) => format!(
                "{}[{}] struct of {}",
                p.name,
                p.array_index,
                fields.items.len()
            ),
            other => format!("{}[{}] {other:?}", p.name, p.array_index),
        })
        .collect()
}

#[test]
fn dlc_name_parts_resolve() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let mut named = 0;
    for (id, set) in db.sets.iter().filter(|(id, _)| **id != 0) {
        for category in [AssetCategory::WeaponParts, AssetCategory::ItemParts] {
            for path in set.paths(category) {
                if matches!(
                    db.object(path),
                    Some(ObjectInfo::Part {
                        part_name: Some(_),
                        ..
                    })
                ) {
                    named += 1;
                    // The stored value leads back to the same path.
                    let value = dlc_value(db, path, category, *id);
                    assert_eq!(
                        db.path_with(category, value, *id, DlcResolution::Verified),
                        Some(path)
                    );
                    assert_eq!(db.path(category, value, *id), None, "unverified policy");
                }
            }
        }
    }
    println!("DLC parts with a name: {named}");
    assert!(named > 300, "{named}");
    let name = |path: &str| match db.object(path) {
        Some(ObjectInfo::Part { part_name, .. }) => part_name.clone(),
        _ => None,
    };
    assert_eq!(
        name("GD_Gladiolus_Weapons.Name.Title.Title_Pearlescent_Unforgiven").as_deref(),
        Some("Unforgiven")
    );
    assert_eq!(
        name("GD_Weap_Pistol.Name.Title_Maliwan.Title_Legendary_ThunderballFists").as_deref(),
        Some("Thunderball Fists")
    );
    // Base precedence: adding the DLC packages changes nothing about base-game objects.
    for (path, info) in &g.base_db.objects {
        assert_eq!(db.object(path), Some(info), "{path}");
    }
    assert_eq!(db.sets[&0], g.base_db.sets[&0]);
}

/// The DLC counterpart of the base census: which rarity values the DLC definitions carry.
#[test]
fn dlc_rarity_census() {
    let Some(g) = game() else { return };
    let db = &g.db;
    let mut by_value: BTreeMap<i64, Vec<(u32, AssetCategory, &str)>> = BTreeMap::new();
    let mut unresolvable = Vec::new();
    for (id, set) in db.sets.iter().filter(|(id, _)| **id != 0) {
        for category in AssetCategory::ALL {
            for path in set.paths(category) {
                // Objects the base game also lists belong to the base census.
                if g.base_db.object(path).is_some() {
                    continue;
                }
                match db.object(path).and_then(ObjectInfo::rarity) {
                    Some(RarityValue::Value(v)) => {
                        assert_eq!(v.fract(), 0.0, "{path}");
                        let entry = by_value.entry(v as i64).or_default();
                        if !entry.iter().any(|e| e.2 == path) {
                            entry.push((*id, category, path));
                        }
                    }
                    Some(RarityValue::Unresolvable) => unresolvable.push(path),
                    Some(RarityValue::None) | None => {}
                }
            }
        }
    }
    for (value, objects) in &by_value {
        let (id, category, path) = objects[0];
        let row = db
            .color_row(*value)
            .map(|r| format!("{} {}", r.color.hex(), r.rating));
        println!(
            "DLC rarity {value:>4}: {:>3} objects, row {row:?}, e.g. [{id}:{category:?}] {path}",
            objects.len()
        );
    }

    // The distinct values, and every rarity struct resolves.
    assert_eq!(
        by_value.keys().copied().collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 10, 181, 497, 500, 501, 502, 503, 504, 506]
    );
    assert!(unresolvable.is_empty(), "{unresolvable:?}");
    // None falls outside the colour table or in its filler row.
    for value in by_value.keys() {
        assert_ne!(db.rarity_for_level(Some(*value)).color, None, "{value}");
    }

    // Seraph. 497 is carried only by weapon barrels: a very-rare body (4) brings the sum to
    // the Seraph row, 501. Seraph items carry 501 on their type or on one part.
    let is_weapon_part = |e: &(u32, AssetCategory, &str)| e.1 == AssetCategory::WeaponParts;
    assert_eq!(by_value[&497].len(), 15);
    assert!(by_value[&497]
        .iter()
        .all(|e| is_weapon_part(e) && e.2.contains("Barrel")));
    assert_eq!(db.rarity_for_level(Some(497 + 4)).tier, RarityTier::Seraph);
    assert!(by_value[&501]
        .iter()
        .all(|e| e.2.contains("Seraph") || e.2.contains("Boss_Ahab")));

    // Pearlescent. 500 is carried only by weapon bodies in the two upgrade packs (sets 8 and
    // 10). None has "Pearl" in its path, but every weapon that uses one of those bodies takes
    // its title from a `Title_Pearlescent_…` name part (checked below).
    let pearl: BTreeSet<&str> = by_value[&500].iter().map(|e| e.2).collect();
    assert_eq!(pearl.len(), 12);
    assert!(by_value[&500]
        .iter()
        .all(|e| is_weapon_part(e) && e.2.contains("_Body_") && [8, 10].contains(&e.0)));
    assert!(!pearl.iter().any(|p| p.contains("Pearl")));
    let mut used = BTreeSet::new();
    let mut weapons = 0;
    for name in ["Gladiolus_Startup_SF.upk", "Lobelia_Startup_SF.upk"] {
        let pkg = g.packages.iter().find(|p| p.name == name).unwrap();
        // The parts a weapon's part list allows in one slot.
        let slot = |list: &PropertyList, slot: &str| -> Vec<String> {
            let Some(parts) = list
                .struct_fields(slot)
                .and_then(|s| s.get("WeightedParts"))
            else {
                return Vec::new();
            };
            let parts = pkg.struct_array(parts).unwrap();
            parts
                .iter()
                .filter_map(|p| pkg.path_of(p.object("Part")?).ok())
                .collect()
        };
        for &index in pkg.exports_of_class("WeaponPartListCollectionDefinition") {
            let list = pkg.properties_of_export(index).unwrap();
            let bodies = slot(&list, "BodyPartData");
            if !bodies.iter().any(|b| pearl.contains(b.as_str())) {
                continue;
            }
            weapons += 1;
            let what = pkg.export_path(index).unwrap();
            // Such a weapon always has a 500 body, and its barrels carry no rarity of their
            // own, so its level is exactly 500.
            assert!(bodies.iter().all(|b| pearl.contains(b.as_str())), "{what}");
            used.extend(bodies.iter().map(|b| *pearl.get(b.as_str()).unwrap()));
            let barrels = slot(&list, "BarrelPartData");
            assert!(!barrels.is_empty(), "{what}");
            for barrel in barrels {
                let barrel_props = props(pkg, &barrel);
                assert_eq!(barrel_props.get("Rarity"), None, "{barrel}");
                let titles = pkg
                    .object_array(barrel_props.get("TitleList").unwrap())
                    .unwrap();
                assert!(!titles.is_empty(), "{barrel}");
                for title in titles {
                    let title = pkg.path_of(title).unwrap();
                    assert!(title.contains(".Title_Pearlescent_"), "{what}: {title}");
                }
            }
        }
    }
    println!("weapons with a 500 body: {weapons}, all titled by a Title_Pearlescent_ name part");
    assert_eq!(used, pearl);
    assert_eq!(db.rarity_for_level(Some(500)).tier, RarityTier::Pearlescent);

    // Rainbow (effervescent): 506 directly, or 502 on a barrel plus a very-rare body.
    assert!(by_value[&506].iter().all(|e| e.0 == 22));
    assert_eq!(db.rarity_for_level(Some(502 + 4)).tier, RarityTier::Rainbow);
    // The five definitions that set both a constant and the effervescent attribute.
    for path in [
        "GD_Anemone_Weapons.Body.SG_Body_Torgue_7",
        "GD_Anemone_Weapons.SMG.SMG_Body_Tediore_6_Infection_Cleaner",
        "GD_Anemone_Weapons.AssaultRifle.AR_Body_Dahl_6_Worming",
        "GD_BanditTech_Items_Anemone.BanditTech.Skin_Effer",
        "GD_Runner_Items_Anemone.Runner.Skin_Effer",
    ] {
        let pkg = g
            .packages
            .iter()
            .find(|p| p.name == "Anemone_Startup_SF.upk")
            .unwrap();
        let list = props(pkg, path);
        let rarity = list
            .struct_fields("Rarity")
            .or(list.struct_fields("BaseRarity"))
            .unwrap();
        assert!(
            rarity.float("BaseValueConstant").is_some_and(|c| c != 0.0),
            "{path}"
        );
        let attribute = pkg
            .path_of(rarity.object("BaseValueAttribute").unwrap())
            .unwrap();
        assert!(
            attribute.ends_with("Anemone_ItemRarity7_Effervescent"),
            "{path}: {attribute}"
        );
        assert_eq!(
            db.object(path).and_then(ObjectInfo::rarity),
            Some(RarityValue::Value(506.0)),
            "{path}"
        );
    }
}

/// In the base game no rarity struct sets both a constant and an attribute, so reading the
/// attribute as replacing the constant changes no base-game value.
#[test]
fn base_rarity_structs_never_set_both_constant_and_attribute() {
    let Some(g) = game() else { return };
    let pkg = startup(g);
    let (mut constants, mut attributes) = (0, 0);
    for path in g.base_db.objects.keys() {
        let list = props(pkg, path);
        for key in ["Rarity", "BaseRarity"] {
            let Some(rarity) = list.struct_fields(key) else {
                continue;
            };
            let constant = rarity.float("BaseValueConstant").is_some_and(|c| c != 0.0);
            let attribute = rarity.object("BaseValueAttribute").is_some_and(|r| r != 0);
            assert!(!(constant && attribute), "{path}");
            constants += usize::from(constant);
            attributes += usize::from(attribute);
        }
    }
    println!("base rarity structs: {constants} with a constant, {attributes} with an attribute, none with both");
    assert!(constants > 0 && attributes > 0);
}

/// Item classes in DLC tables. `CrossDLCClassModDefinition` (which extends `ClassModDefinition`
/// in WillowGame.upk) is the only gear class the base game does not have.
#[test]
fn dlc_item_type_classes() {
    let Some(g) = game() else { return };
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    for set in g.db.sets.values() {
        for path in set.paths(AssetCategory::ItemTypes) {
            if let Some(ObjectInfo::ItemType { class_name, .. }) = g.db.object(path) {
                *classes.entry(class_name).or_default() += 1;
            }
        }
    }
    println!("item type classes: {classes:?}");
    let known = [
        "ArtifactDefinition",
        "ClassModDefinition",
        "CrossDLCClassModDefinition",
        "GrenadeModDefinition",
        "MissionItemDefinition",
        "ShieldDefinition",
        "UsableCustomizationItemDefinition",
        "UsableItemDefinition",
    ];
    let other: Vec<&&str> = classes.keys().filter(|c| !known.contains(c)).collect();
    // Two Anemone table entries point at objects that are not item types at all.
    assert_eq!(other, [&"ItemPoolDefinition", &"ObjectRedirector"]);
    assert_eq!(classes["CrossDLCClassModDefinition"], 24);
}

/// DLC items built from the tables. They show the resolution working end to end on real data,
/// under the two hypotheses; they cannot show that the hypotheses are right.
#[test]
fn constructed_dlc_items_resolve_only_when_asked_to() {
    let Some(g) = game() else { return };
    let db = &g.db;
    use AssetCategory::{Balances, ItemParts, ItemTypes, Manufacturers, WeaponParts, WeaponTypes};
    let verified = DlcResolution::Verified;

    // The Unforgiven, a pearlescent Jakobs pistol from set 8 (Gladiolus).
    let part8 = |path| Some(dlc_value(db, path, WeaponParts, 8));
    let unforgiven = ItemRefs {
        is_weapon: true,
        set: Some(8),
        item_type: Some(base_value(
            db,
            "GD_Weap_Pistol.A_Weapons.WeaponType_Jakobs_Pistol",
            WeaponTypes,
        )),
        balance: Some(dlc_value(
            db,
            "GD_Gladiolus_Weapons.Pistol.Pistol_Jakobs_6_Unforgiven",
            Balances,
            8,
        )),
        manufacturer: Some(base_value(
            db,
            "GD_Manufacturers.Manufacturers.Jakobs",
            Manufacturers,
        )),
        parts: [
            part8("GD_Gladiolus_Weapons.Pistol.Pistol_Body_Jakobs_6"),
            Some(base_value(
                db,
                "GD_Weap_Pistol.Grip.Pistol_Grip_Jakobs",
                WeaponParts,
            )),
            part8("GD_Gladiolus_Weapons.Pistol.Pistol_Barrel_Jakobs_Unforgiven"),
            Some(W),
            Some(W),
            Some(W),
            Some(W),
            Some(W),
            part8("GD_Gladiolus_Weapons.ManufacturerMaterials.Mat_Jakobs_6_Unforgiven"),
            Some(W),
            part8("GD_Gladiolus_Weapons.Name.Title.Title_Pearlescent_Unforgiven"),
        ],
    };
    assert_eq!(db.info(&unforgiven), ItemInfo::UNKNOWN);
    assert_eq!(db.gear_class(&unforgiven), GearClass::Unknown);
    assert_eq!(
        db.info_with(&unforgiven, verified),
        ItemInfo {
            name: Some("Unforgiven".into()),
            rarity: RarityInfo {
                level: Some(500),
                tier: RarityTier::Pearlescent,
                color: Some(Rgba::new(0x00, 0xFF, 0xFF, 0)),
            },
            manufacturer: Some("Jakobs".into()),
            category: ItemCategory::Weapon(WeaponKind::Pistol),
            gear_class: GearClass::Gear,
        }
    );

    // The Devastator, a seraph Torgue pistol from set 1 (Orchid): a base very-rare body (4)
    // plus the DLC barrel (497).
    let part1 = |path| Some(dlc_value(db, path, WeaponParts, 1));
    let devastator = ItemRefs {
        is_weapon: true,
        set: Some(1),
        item_type: Some(base_value(
            db,
            "GD_Weap_Pistol.A_Weapons.WeaponType_Torgue_Pistol",
            WeaponTypes,
        )),
        balance: Some(dlc_value(
            db,
            "GD_Orchid_RaidWeapons.Pistol.Devastator.Orchid_Seraph_Devastator_Balance",
            Balances,
            1,
        )),
        manufacturer: Some(base_value(
            db,
            "GD_Manufacturers.Manufacturers.Torgue",
            Manufacturers,
        )),
        parts: [
            Some(base_value(
                db,
                "GD_Weap_Pistol.Body.Pistol_Body_Torgue_4",
                WeaponParts,
            )),
            Some(W),
            part1("GD_Orchid_RaidWeapons.Pistol.Devastator.Orchid_Seraph_Devastator_Barrel"),
            Some(W),
            Some(W),
            Some(W),
            Some(W),
            Some(W),
            part1("GD_Orchid_RaidWeapons.ManufacturerMaterials.Mat_Torgue_1_Devastator"),
            Some(W),
            part1("GD_Orchid_RaidWeapons.Pistol.Devastator.Orchid_Seraph_Devastator_Title"),
        ],
    };
    assert_eq!(db.info(&devastator), ItemInfo::UNKNOWN);
    let info = db.info_with(&devastator, verified);
    assert_eq!(info.name.as_deref(), Some("Devastator"));
    assert_eq!(
        info.rarity,
        RarityInfo {
            level: Some(501),
            tier: RarityTier::Seraph,
            color: Some(Rgba::new(0xFF, 0x9A, 0xB8, 0))
        }
    );
    assert_eq!(info.manufacturer.as_deref(), Some("Torgue"));
    assert_eq!(info.gear_class, GearClass::Gear);

    // A Mechromancer class mod from set 9 (Aster), of the class only DLC has.
    let necromancer = ItemRefs {
        is_weapon: false,
        set: Some(9),
        item_type: Some(dlc_value(
            db,
            "GD_Aster_ClassMods.A_Item_Mechromancer.ClassMod_Mechromancer_Necromancer",
            ItemTypes,
            9,
        )),
        balance: None,
        manufacturer: None,
        parts: [Some(I); PART_SLOTS],
    };
    assert_eq!(db.info(&necromancer), ItemInfo::UNKNOWN);
    let info = db.info_with(&necromancer, verified);
    assert_eq!(
        (info.category, info.gear_class),
        (ItemCategory::ClassMod, GearClass::Gear)
    );
    // With no title part there is no name; with a DLC title there is.
    assert_eq!(info.name, None);
    let title = db.sets[&9]
        .paths(ItemParts)
        .find(|p| {
            matches!(
                db.object(p),
                Some(ObjectInfo::Part {
                    part_name: Some(_),
                    ..
                })
            )
        })
        .unwrap();
    let mut named = necromancer.clone();
    named.parts[10] = Some(dlc_value(db, title, ItemParts, 9));
    assert!(db.info_with(&named, verified).name.is_some());

    // A set with no asset libraries (7, a skin pack): flagged references resolve to nothing.
    let mut no_tables = unforgiven.clone();
    no_tables.set = Some(7);
    let info = db.info_with(&no_tables, verified);
    assert_eq!((info.name, info.rarity), (None, RarityInfo::UNKNOWN));
    assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));
}

// MARK: Import

#[test]
fn imports_real_game_within_time_limits() {
    let Some(g) = game() else { return };
    let tmp = TempDir::new();
    let cache = DatabaseCache::new(&tmp.0, &g.install).unwrap();
    let never = AtomicBool::new(false);

    let mut seen: Vec<ImportProgress> = Vec::new();
    let start = Instant::now();
    let db = load_or_import(&g.install, &cache, &mut |p| seen.push(p), &never).unwrap();
    let first = start.elapsed();
    assert!(first < Duration::from_secs(30), "{first:?}");
    assert_eq!(
        seen.first().map(|p| &p.phase),
        Some(&ImportPhase::Fingerprinting)
    );
    assert_eq!(
        seen.last(),
        Some(&ImportProgress {
            phase: ImportPhase::Done,
            fraction: 1.0
        })
    );
    assert!(seen.windows(2).all(|w| w[0].fraction <= w[1].fraction));
    let read: Vec<&str> = seen
        .iter()
        .filter_map(|p| match &p.phase {
            ImportPhase::ReadingPackage(name) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(read.len(), 22);
    assert_eq!(read[0], "Startup.upk");

    assert_eq!(db.fingerprint, g.install.fingerprint().unwrap());
    assert_eq!(db.schema, ItemDatabase::SCHEMA_VERSION);
    assert_eq!(db.sets.len(), 14);
    assert!(db.skipped_packages.is_empty());
    assert_eq!(
        db.info(&known_items()[0].refs).name.as_deref(),
        Some("Righteous Infinity")
    );
    // The same database as one built straight from the packages.
    assert_eq!(db.sets, g.db.sets);
    assert_eq!(db.objects, g.db.objects);

    // A cache hit: equal, fast, and without reading any package.
    let mut again = Vec::new();
    let start = Instant::now();
    let cached = load_or_import(&g.install, &cache, &mut |p| again.push(p.phase), &never).unwrap();
    let second = start.elapsed();
    assert!(second < Duration::from_secs(2), "{second:?}");
    assert_eq!(cached, db);
    assert_eq!(again, [ImportPhase::Fingerprinting, ImportPhase::Done]);

    let cache_file = tmp.0.join(DatabaseCache::file_name(&db.fingerprint));
    let size = std::fs::metadata(&cache_file).unwrap().len();
    println!("import: first run {first:?}, cache hit {second:?}, cache file {size} bytes");
    println!(
        "objects {}, sets {:?}",
        db.objects.len(),
        db.sets.keys().collect::<Vec<_>>()
    );
}

/// Every folder under the install's package folders, and every file with its size and
/// modification time.
fn snapshot(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = vec![
        root.join("WillowGame").join("CookedPCConsole"),
        root.join("DLC"),
    ];
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

#[test]
fn import_never_writes_to_the_game_folder() {
    let Some(g) = game() else { return };
    let tmp = TempDir::new();
    let cache = DatabaseCache::new(&tmp.0, &g.install).unwrap();
    let before = snapshot(g.install.root());
    assert!(before.len() > 1000);
    load_or_import(&g.install, &cache, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    import(&g.install, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let after = snapshot(g.install.root());
    assert!(after == before, "the game folder changed during an import");
}

#[test]
fn canceled_real_import_leaves_no_cache() {
    let Some(g) = game() else { return };
    let tmp = TempDir::new();
    let cache = DatabaseCache::new(&tmp.0, &g.install).unwrap();
    let cache_files = || std::fs::read_dir(&tmp.0).unwrap().count();

    let canceled = AtomicBool::new(true);
    assert_eq!(
        load_or_import(&g.install, &cache, &mut |_| {}, &canceled),
        Err(GameDataError::Canceled)
    );
    assert_eq!(cache_files(), 0);

    // Canceled while reading the DLC packages, and once the build has started.
    for stop_at in [
        ImportPhase::ReadingPackage("Tulip_Startup_SF.upk".into()),
        ImportPhase::Building,
    ] {
        let cancel = AtomicBool::new(false);
        let mut progress = |p: ImportProgress| {
            if p.phase == stop_at {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        assert_eq!(
            load_or_import(&g.install, &cache, &mut progress, &cancel),
            Err(GameDataError::Canceled)
        );
        assert_eq!(cache_files(), 0, "{stop_at:?}");
    }
}
