//! The item database built from the game's packages: asset tables per set, the definitions
//! those tables refer to, and the rarity colour table.

mod codec;
mod item_info;
mod rarity;
mod stat_data;
pub(crate) mod stats;

use std::collections::{BTreeMap, HashMap};
use std::fmt;

pub use item_info::{
    GearClass, ItemCategory, ItemInfo, ItemRefs, RarityInfo, RarityTier, PART_SLOTS,
};
pub use rarity::{counts_part_slots, rarity_level};
pub use stat_data::{
    AttributeEffect, AttributeSource, BaseValueMode, CardAttribute, CardText, Comparison,
    Condition, Conditional, ConditionalCase, CustomPresentation, Formula, Initialization,
    ModifierKind, Presentation, PresentationEntries, PresentationList, PresentationRule, Remap,
    Rounding, SignStyle, SlotEffect, SlotUpgrade, StatDefinition, StatsData, UiStat, ValueData,
    WeaponBase,
};
pub use stats::{CardLine, CardLineKind, ItemLevel, ItemStats, StatKind, StatRow, StatValue};

/// The six asset libraries, in `GD_Globals.General.Globals.AssetLibraries[0…5]` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetCategory {
    WeaponTypes,
    WeaponParts,
    ItemTypes,
    ItemParts,
    Manufacturers,
    Balances,
}

impl AssetCategory {
    pub const ALL: [AssetCategory; 6] = [
        Self::WeaponTypes,
        Self::WeaponParts,
        Self::ItemTypes,
        Self::ItemParts,
        Self::Manufacturers,
        Self::Balances,
    ];

    /// Low bits of a saved value that index the asset within a sublibrary (format doc §6.6).
    pub const fn asset_bits(self) -> u32 {
        match self {
            Self::WeaponTypes => 6,
            Self::WeaponParts => 11,
            Self::ItemTypes => 8,
            Self::ItemParts => 10,
            Self::Manufacturers => 7,
            Self::Balances => 10,
        }
    }

    /// High bits of a saved value that index the sublibrary (format doc §6.6). For an item of a
    /// DLC set, the top one of these bits selects that set's tables (see [`DlcResolution`]).
    pub const fn lib_bits(self) -> u32 {
        match self {
            Self::WeaponTypes => 7,
            Self::WeaponParts => 6,
            Self::ItemTypes => 9,
            Self::ItemParts => 6,
            Self::Manufacturers => 4,
            Self::Balances => 10,
        }
    }

    /// The all-ones value of this category's field, which marks "nothing here".
    pub const fn all_ones(self) -> u32 {
        (1 << (self.asset_bits() + self.lib_bits())) - 1
    }

    /// Splits a saved value into its sublibrary index and asset index.
    pub const fn split(self, value: u32) -> (u32, u32) {
        (
            value >> self.asset_bits(),
            value & ((1 << self.asset_bits()) - 1),
        )
    }

    /// The lib-index bit that, for an item of a DLC set, marks a reference into that set.
    pub const fn dlc_flag(self) -> u32 {
        1 << (self.lib_bits() - 1)
    }

    pub const fn types(is_weapon: bool) -> Self {
        if is_weapon {
            Self::WeaponTypes
        } else {
            Self::ItemTypes
        }
    }

    pub const fn parts(is_weapon: bool) -> Self {
        if is_weapon {
            Self::WeaponParts
        } else {
            Self::ItemParts
        }
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

/// A colour as the game stores it. Alpha is reported as stored; it is not a "hidden" marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `#RRGGBB`, uppercase; alpha is not included.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

/// A resolved `AttributeInitializationData` value (a definition's `Rarity` or `BaseRarity`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RarityValue {
    /// The definition has no such struct.
    None,
    Value(f64),
    /// The struct exists but its value cannot be determined statically.
    Unresolvable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponKind {
    Pistol,
    Shotgun,
    Smg,
    AssaultRifle,
    SniperRifle,
    Launcher,
}

/// What the database keeps of one definition that an asset table refers to.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectInfo {
    WeaponType {
        kind: Option<WeaponKind>,
        type_name: Option<String>,
        base_rarity: RarityValue,
    },
    ItemType {
        class_name: String,
        item_name: Option<String>,
        name_is_full: bool,
        base_rarity: RarityValue,
    },
    Part {
        class_name: String,
        part_name: Option<String>,
        rarity: RarityValue,
    },
    Manufacturer {
        display_name: Option<String>,
    },
    Balance {
        class_name: String,
    },
}

impl ObjectInfo {
    /// The rarity this definition contributes, for types and parts.
    pub fn rarity(&self) -> Option<RarityValue> {
        match self {
            Self::WeaponType { base_rarity, .. } | Self::ItemType { base_rarity, .. } => {
                Some(*base_rarity)
            }
            Self::Part { rarity, .. } => Some(*rarity),
            Self::Manufacturer { .. } | Self::Balance { .. } => None,
        }
    }
}

/// One row of `GD_Globals.General.Globals.RarityLevelColors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RarityColorRow {
    pub min_level: i32,
    pub max_level: i32,
    pub color: Rgba,
    /// The row's `EItemRarity` name, e.g. `RARITY_Legendary`.
    pub rating: String,
}

/// One sublibrary: object paths by asset index. `None` marks an entry with no path.
pub type AssetTable = Vec<Option<String>>;

/// The asset tables of one set (0 is the base game, otherwise a DLC's `PackageId`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetSet {
    /// The DLC's internal name (e.g. `Tulip`) for DLC sets; `None` for the base game.
    pub dlc_name: Option<String>,
    tables: [Vec<AssetTable>; 6],
    unresolved: [Vec<String>; 6],
}

impl AssetSet {
    pub fn new(dlc_name: Option<String>) -> Self {
        Self {
            dlc_name,
            ..Self::default()
        }
    }

    /// A category's tables, indexed `[lib][asset]`. A sublibrary whose link is null has an
    /// empty table at its lib index.
    pub fn tables(&self, category: AssetCategory) -> &[AssetTable] {
        &self.tables[category.index()]
    }

    pub fn set_tables(&mut self, category: AssetCategory, tables: Vec<AssetTable>) {
        self.tables[category.index()] = tables;
    }

    /// Names of the category's sublibraries whose link is null or cannot be located.
    pub fn unresolved_sublibraries(&self, category: AssetCategory) -> &[String] {
        &self.unresolved[category.index()]
    }

    pub fn set_unresolved_sublibraries(&mut self, category: AssetCategory, names: Vec<String>) {
        self.unresolved[category.index()] = names;
    }

    /// Every path in a category's tables.
    pub fn paths(&self, category: AssetCategory) -> impl Iterator<Item = &str> {
        self.tables(category)
            .iter()
            .flatten()
            .flatten()
            .map(String::as_str)
    }
}

/// Whether references into DLC sets (items whose set id is not 0) are resolved.
///
/// How a DLC item's numbers map onto the DLC's tables rests on two hypotheses that no real DLC
/// item has yet been compared against in game:
///
/// - **H-set:** an item's set id is the `PackageId` of the DLC whose asset-library set holds its
///   DLC assets.
/// - **H-flag:** for an item whose set id is not 0, a reference whose lib index has its top bit
///   set ([`AssetCategory::dlc_flag`]) indexes that set's tables with the bit cleared; any other
///   reference indexes the base game's tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DlcResolution {
    /// DLC-set items resolve to nothing: every field unknown, never "gear".
    Unverified,
    /// DLC-set items resolve through H-set and H-flag.
    Verified,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemDatabase {
    pub schema: u32,
    /// Identifies the package files this database was built from (see `GameInstall::fingerprint`).
    pub fingerprint: String,
    /// Asset tables by set id: 0 is the base game, the others are DLC `PackageId`s.
    pub sets: BTreeMap<u32, AssetSet>,
    /// Definitions by object path, for every table path found in the packages.
    pub objects: HashMap<String, ObjectInfo>,
    pub rarity_colors: Vec<RarityColorRow>,
    /// DLC packages that could not be read or whose asset-library set was unusable. They are
    /// left out instead of failing the import.
    pub skipped_packages: Vec<String>,
    /// What item cards are computed from (see [`ItemDatabase::stats`]).
    pub stat_data: StatsData,
}

impl ItemDatabase {
    /// Version 3 added the stat data; version 4 the card lines (presentation details,
    /// card attributes, conditional initialisation, damage types).
    pub const SCHEMA_VERSION: u32 = 4;

    /// The policy [`ItemDatabase::path`], [`ItemDatabase::info`] and [`ItemDatabase::gear_class`]
    /// follow. It stays `Unverified` until a real DLC item has been compared in game.
    pub const DLC_RESOLUTION: DlcResolution = DlcResolution::Unverified;

    pub fn new(fingerprint: impl Into<String>) -> Self {
        Self {
            schema: Self::SCHEMA_VERSION,
            fingerprint: fingerprint.into(),
            sets: BTreeMap::new(),
            objects: HashMap::new(),
            rarity_colors: Vec::new(),
            skipped_packages: Vec::new(),
            stat_data: StatsData::default(),
        }
    }

    /// The object path a saved value refers to, under [`ItemDatabase::DLC_RESOLUTION`].
    pub fn path(&self, category: AssetCategory, value: u32, set: u32) -> Option<&str> {
        self.path_with(category, value, set, Self::DLC_RESOLUTION)
    }

    /// The object path a saved value of an item in `set` refers to.
    ///
    /// Set 0 always indexes the base game's tables. Any other set resolves only with
    /// [`DlcResolution::Verified`], by H-flag: the top lib bit selects that set's tables.
    pub fn path_with(
        &self,
        category: AssetCategory,
        value: u32,
        set: u32,
        dlc: DlcResolution,
    ) -> Option<&str> {
        let (mut lib, asset) = category.split(value);
        let mut source = 0;
        if set != 0 {
            if dlc == DlcResolution::Unverified {
                return None;
            }
            if lib & category.dlc_flag() != 0 {
                lib &= !category.dlc_flag();
                source = set;
            }
        }
        let tables = self.sets.get(&source)?.tables(category);
        tables.get(lib as usize)?.get(asset as usize)?.as_deref()
    }

    /// The saved value that refers to `path` from `set`'s own tables (the inverse of
    /// [`ItemDatabase::path_with`]), if representable. For a DLC set the value carries the H-flag
    /// bit, so it is what an item of that set would store.
    pub fn value_of(&self, path: &str, category: AssetCategory, set: u32) -> Option<u32> {
        let tables = self.sets.get(&set)?.tables(category);
        let flag = if set == 0 { 0 } else { category.dlc_flag() };
        let lib_limit = if set == 0 {
            1 << category.lib_bits()
        } else {
            category.dlc_flag()
        };
        for (lib, table) in tables.iter().enumerate() {
            let Some(asset) = table.iter().position(|p| p.as_deref() == Some(path)) else {
                continue;
            };
            let (lib, asset) = (u32::try_from(lib).ok()?, u32::try_from(asset).ok()?);
            if lib >= lib_limit || asset >= 1 << category.asset_bits() {
                return None;
            }
            return Some((lib | flag) << category.asset_bits() | asset);
        }
        None
    }

    pub fn object(&self, path: &str) -> Option<&ObjectInfo> {
        self.objects.get(path)
    }

    /// The first `RarityLevelColors` row whose level range contains `level`.
    pub fn color_row(&self, level: i64) -> Option<&RarityColorRow> {
        self.rarity_colors
            .iter()
            .find(|row| i64::from(row.min_level) <= level && level <= i64::from(row.max_level))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_bits() {
        let all = AssetCategory::ALL;
        assert_eq!(all.map(AssetCategory::asset_bits), [6, 11, 8, 10, 7, 10]);
        assert_eq!(all.map(AssetCategory::lib_bits), [7, 6, 9, 6, 4, 10]);
        // The field widths of format doc §6.6: weapon type 13, weapon part 17, item type 17,
        // item part 16, manufacturer 11, balance 20.
        assert_eq!(
            all.map(AssetCategory::all_ones),
            [8191, 131071, 131071, 65535, 2047, 1048575]
        );
        assert_eq!(all.map(AssetCategory::dlc_flag), [64, 32, 256, 32, 8, 512]);
        assert_eq!(all.map(AssetCategory::index), [0, 1, 2, 3, 4, 5]);
        assert_eq!(AssetCategory::types(true), AssetCategory::WeaponTypes);
        assert_eq!(AssetCategory::types(false), AssetCategory::ItemTypes);
        assert_eq!(AssetCategory::parts(true), AssetCategory::WeaponParts);
        assert_eq!(AssetCategory::parts(false), AssetCategory::ItemParts);
        // The README vectors: balance 131111 is (128, 39), weapon type 1924 is (30, 4).
        assert_eq!(AssetCategory::Balances.split(131111), (128, 39));
        assert_eq!(AssetCategory::WeaponTypes.split(1924), (30, 4));
        assert_eq!(AssetCategory::Manufacturers.split(133), (1, 5));
        assert_eq!(AssetCategory::ItemTypes.split(2840), (11, 24));
        assert_eq!(Rgba::new(0x3D, 0xD2, 0x0B, 0xFF).hex(), "#3DD20B");
        assert_eq!(Rgba::new(0, 0, 0, 0).to_string(), "#000000");
    }

    #[test]
    fn paths_and_values() {
        use AssetCategory::{Balances, Manufacturers};
        let mut base = AssetSet::new(None);
        base.set_tables(
            Balances,
            vec![vec![Some("GD_X.A.Bal".into()), None], vec![]],
        );
        base.set_tables(Manufacturers, vec![vec![Some("GD_M.M.Vladof".into())]]);
        let mut dlc = AssetSet::new(Some("Orchid".into()));
        dlc.set_tables(
            Balances,
            vec![vec![], vec![None, Some("GD_O.A.Bal".into())]],
        );
        let mut db = ItemDatabase::new("abc");
        db.sets.insert(0, base);
        db.sets.insert(1, dlc);
        db.rarity_colors.push(RarityColorRow {
            min_level: 7,
            max_level: 10,
            color: Rgba::new(255, 180, 0, 255),
            rating: "RARITY_Legendary".into(),
        });

        assert_eq!(db.path(Balances, 0, 0), Some("GD_X.A.Bal"));
        assert_eq!(db.path(Balances, 1, 0), None); // an entry with no path
        assert_eq!(db.path(Balances, 2, 0), None); // past the table
        assert_eq!(db.path(Balances, 1 << 10, 0), None); // an empty lib
        assert_eq!(db.path(Balances, 2 << 10, 0), None); // past the libs
        assert_eq!(db.path(Balances, u32::MAX, 0), None);
        assert_eq!(db.path(Balances, 0, 1), None); // DLC sets are unverified
        assert_eq!(db.value_of("GD_M.M.Vladof", Manufacturers, 0), Some(0));
        assert_eq!(db.value_of("nope", Manufacturers, 0), None);
        assert_eq!(db.value_of("GD_M.M.Vladof", Manufacturers, 5), None);

        // Set 1, lib 1, asset 1, with the flag bit (lib bit 9).
        let value = (512 | 1) << 10 | 1;
        assert_eq!(db.value_of("GD_O.A.Bal", Balances, 1), Some(value));
        let verified = DlcResolution::Verified;
        assert_eq!(
            db.path_with(Balances, value, 1, verified),
            Some("GD_O.A.Bal")
        );
        assert_eq!(
            db.path_with(Balances, value, 1, DlcResolution::Unverified),
            None
        );
        assert_eq!(db.path_with(Balances, 512 << 10, 1, verified), None); // the set's empty lib 0
        assert_eq!(db.path_with(Balances, 0, 1, verified), Some("GD_X.A.Bal")); // base, unflagged
        assert_eq!(db.path_with(Balances, 0, 0, verified), Some("GD_X.A.Bal"));

        assert_eq!(
            db.color_row(9).map(|r| r.color.hex()).as_deref(),
            Some("#FFB400")
        );
        assert_eq!(db.color_row(6), None);
        assert_eq!(db.color_row(i64::MAX), None);
        assert_eq!(db.object("GD_X.A.Bal"), None);
    }

    #[test]
    fn value_of_refuses_unrepresentable_positions() {
        use AssetCategory::Manufacturers;
        // Manufacturers have 4 lib bits: 16 libs for the base game, 8 for a DLC set.
        let far = |libs: usize| {
            let mut tables = vec![vec![]; libs];
            tables.push(vec![Some("P".to_string())]);
            tables
        };
        let mut db = ItemDatabase::new("x");
        let mut base = AssetSet::new(None);
        base.set_tables(Manufacturers, far(15));
        db.sets.insert(0, base.clone());
        assert_eq!(db.value_of("P", Manufacturers, 0), Some(15 << 7));
        base.set_tables(Manufacturers, far(16));
        db.sets.insert(0, base);
        assert_eq!(db.value_of("P", Manufacturers, 0), None);

        let mut dlc = AssetSet::new(None);
        dlc.set_tables(Manufacturers, far(7));
        db.sets.insert(3, dlc.clone());
        assert_eq!(db.value_of("P", Manufacturers, 3), Some((8 | 7) << 7));
        dlc.set_tables(Manufacturers, far(8));
        db.sets.insert(3, dlc.clone());
        assert_eq!(db.value_of("P", Manufacturers, 3), None);
        // An asset index past the asset bits.
        let mut wide = vec![None; 128];
        wide.push(Some("Q".to_string()));
        dlc.set_tables(Manufacturers, vec![wide]);
        db.sets.insert(3, dlc);
        assert_eq!(db.value_of("Q", Manufacturers, 3), None);
    }
}
