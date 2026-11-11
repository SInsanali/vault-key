//! Resolves a saved item's numbers to what the game shows: name, rarity, manufacturer,
//! category, and whether it is gear.

use super::rarity::{counts_part_slots, rarity_level};
use super::{
    AssetCategory, DlcResolution, ItemDatabase, ObjectInfo, RarityValue, Rgba, WeaponKind,
};

/// The number of part slots in a saved item (format doc §6.6).
pub const PART_SLOTS: usize = 11;
/// Part slots holding the name parts.
const PREFIX_SLOT: usize = 9;
const TITLE_SLOT: usize = 10;

/// Transparent black: the colour of `RarityLevelColors` rows that cover unused levels.
const FILLER_COLOR: Rgba = Rgba::new(0, 0, 0, 0);

/// The decoded numbers of one saved item or weapon (format doc §6.6). A field is `None` when
/// the item's bitstream ends before it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemRefs {
    /// Weapons and items use different type and part libraries and field widths.
    pub is_weapon: bool,
    /// The asset-library set: 0 for the base game, a DLC's `PackageId`, or 255 for the
    /// placeholder "virtual" items.
    pub set: Option<u32>,
    pub item_type: Option<u32>,
    pub balance: Option<u32>,
    pub manufacturer: Option<u32>,
    /// Weapons: body, grip, barrel, sight, stock, elemental, accessory 1, accessory 2, material,
    /// prefix, title. Items: alpha … theta, material, prefix, title. An all-ones value (131071
    /// for weapons, 65535 for items) is an empty slot.
    pub parts: [Option<u32>; PART_SLOTS],
}

impl ItemRefs {
    /// A placeholder "virtual" item: set 255 and every other reference zero or absent.
    pub fn is_virtual(&self) -> bool {
        let zero = |v: Option<u32>| v.unwrap_or(0) == 0;
        self.set == Some(255)
            && zero(self.item_type)
            && zero(self.balance)
            && zero(self.manufacturer)
            && self.parts.iter().all(|&p| zero(p))
    }
}

/// Whether an item is gear (a weapon, shield, grenade mod, class mod or relic the player can
/// carry and drop) as opposed to a story, mission, usable or cosmetic item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GearClass {
    Gear,
    NotGear,
    /// The item could not be classified. Treat it as not safe to duplicate.
    Unknown,
}

/// What kind of item a saved item is, from its type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemCategory {
    Weapon(WeaponKind),
    Shield,
    GrenadeMod,
    ClassMod,
    Relic,
    Usable,
    Customization,
    Mission,
    Unknown,
}

/// The rarity label shown for an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RarityTier {
    Common,
    Uncommon,
    Rare,
    VeryRare,
    ETech,
    Legendary,
    Seraph,
    Pearlescent,
    Rainbow,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RarityInfo {
    /// The summed rarity level (spike doc §5), or `None` when unknown.
    pub level: Option<i64>,
    pub tier: RarityTier,
    /// The `RarityLevelColors` colour for `level`, alpha as stored. `None` when no row contains
    /// the level or the row is the transparent-black filler, so the UI renders a neutral colour.
    pub color: Option<Rgba>,
}

impl RarityInfo {
    pub const UNKNOWN: Self = Self {
        level: None,
        tier: RarityTier::Unknown,
        color: None,
    };
}

/// What the game shows for a saved item. `None` means unknown; nothing is ever guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub name: Option<String>,
    pub rarity: RarityInfo,
    pub manufacturer: Option<String>,
    pub category: ItemCategory,
    pub gear_class: GearClass,
}

impl ItemInfo {
    pub const UNKNOWN: Self = Self {
        name: None,
        rarity: RarityInfo::UNKNOWN,
        manufacturer: None,
        category: ItemCategory::Unknown,
        gear_class: GearClass::Unknown,
    };
}

/// What a saved part slot refers to.
enum PartSlot<'a> {
    /// Absent, or all-ones.
    Empty,
    Part {
        name: Option<&'a str>,
        rarity: RarityValue,
    },
    /// Non-empty, but not a part in this database.
    Unresolved,
}

/// One item being resolved against the database.
struct Lookup<'a> {
    db: &'a ItemDatabase,
    item: &'a ItemRefs,
    set: u32,
    dlc: DlcResolution,
}

impl<'a> Lookup<'a> {
    fn object(&self, category: AssetCategory, value: Option<u32>) -> Option<&'a ObjectInfo> {
        let path = self.db.path_with(category, value?, self.set, self.dlc)?;
        self.db.object(path)
    }

    /// The item's type definition, when it resolves to the kind matching `is_weapon`.
    fn type_object(&self) -> Option<&'a ObjectInfo> {
        let category = AssetCategory::types(self.item.is_weapon);
        match self.object(category, self.item.item_type)? {
            o @ ObjectInfo::WeaponType { .. } if self.item.is_weapon => Some(o),
            o @ ObjectInfo::ItemType { .. } if !self.item.is_weapon => Some(o),
            _ => None,
        }
    }

    fn part_slot(&self, index: usize) -> PartSlot<'a> {
        let category = AssetCategory::parts(self.item.is_weapon);
        let Some(value) = self.item.parts.get(index).copied().flatten() else {
            return PartSlot::Empty;
        };
        if value == category.all_ones() {
            return PartSlot::Empty;
        }
        match self.object(category, Some(value)) {
            Some(ObjectInfo::Part {
                part_name, rarity, ..
            }) => PartSlot::Part {
                name: part_name.as_deref(),
                rarity: *rarity,
            },
            _ => PartSlot::Unresolved,
        }
    }

    /// The type's base rarity, plus the rarity of each non-empty part slot when the category's
    /// part slots count. An unresolved type, an unknown category, or a non-empty slot that does
    /// not resolve to a part contributes [`RarityValue::Unresolvable`].
    fn contributions(&self) -> Vec<RarityValue> {
        let Some(type_object) = self.type_object() else {
            return vec![RarityValue::Unresolvable];
        };
        let Some(base) = type_object.rarity() else {
            return vec![RarityValue::Unresolvable];
        };
        match counts_part_slots(category_of(Some(type_object))) {
            None => vec![RarityValue::Unresolvable],
            Some(false) => vec![base],
            Some(true) => {
                let mut out = vec![base];
                for index in 0..PART_SLOTS {
                    match self.part_slot(index) {
                        PartSlot::Empty => {}
                        PartSlot::Part { rarity, .. } => out.push(rarity),
                        PartSlot::Unresolved => out.push(RarityValue::Unresolvable),
                    }
                }
                out
            }
        }
    }

    fn name(&self, type_object: Option<&ObjectInfo>, category: ItemCategory) -> Option<String> {
        let mut item_name = None;
        if let Some(ObjectInfo::ItemType {
            item_name: name,
            name_is_full,
            ..
        }) = type_object
        {
            if *name_is_full && name.is_some() {
                return name.clone();
            }
            item_name = name.clone();
        }
        if !counts_part_slots(category)? {
            // Part slots are ignored: customization items carry garbage values there.
            return item_name;
        }
        // A non-empty name slot must be a part with a name; the title is required.
        let name_part = |index: usize| -> Option<Option<&str>> {
            match self.part_slot(index) {
                PartSlot::Empty => Some(None),
                PartSlot::Part {
                    name: Some(name), ..
                } => Some(Some(name)),
                PartSlot::Part { name: None, .. } | PartSlot::Unresolved => None,
            }
        };
        let prefix = name_part(PREFIX_SLOT)?;
        let title = name_part(TITLE_SLOT)??;
        Some(match prefix {
            Some(prefix) => format!("{prefix} {title}"),
            None => title.to_string(),
        })
    }

    fn manufacturer(&self) -> Option<String> {
        match self.object(AssetCategory::Manufacturers, self.item.manufacturer)? {
            ObjectInfo::Manufacturer { display_name } => display_name.clone(),
            _ => None,
        }
    }

    fn gear_class(&self, category: ItemCategory) -> GearClass {
        match category {
            ItemCategory::Weapon(_)
            | ItemCategory::Shield
            | ItemCategory::GrenadeMod
            | ItemCategory::ClassMod
            | ItemCategory::Relic => {
                // Mission weapons are real weapons but belong to a mission. A balance that does
                // not resolve does not change the class.
                match self.object(AssetCategory::Balances, self.item.balance) {
                    Some(ObjectInfo::Balance { class_name })
                        if class_name == "MissionWeaponBalanceDefinition" =>
                    {
                        GearClass::NotGear
                    }
                    _ => GearClass::Gear,
                }
            }
            ItemCategory::Usable | ItemCategory::Customization | ItemCategory::Mission => {
                GearClass::NotGear
            }
            ItemCategory::Unknown => GearClass::Unknown,
        }
    }
}

fn category_of(type_object: Option<&ObjectInfo>) -> ItemCategory {
    match type_object {
        Some(ObjectInfo::WeaponType {
            kind: Some(kind), ..
        }) => ItemCategory::Weapon(*kind),
        Some(ObjectInfo::ItemType { class_name, .. }) => match class_name.as_str() {
            "ShieldDefinition" => ItemCategory::Shield,
            "GrenadeModDefinition" => ItemCategory::GrenadeMod,
            // `CrossDLCClassModDefinition` extends `ClassModDefinition` (WillowGame.upk); the
            // Mechromancer and Psycho class mods of later DLCs use it.
            "ClassModDefinition" | "CrossDLCClassModDefinition" => ItemCategory::ClassMod,
            "ArtifactDefinition" => ItemCategory::Relic,
            "UsableItemDefinition" => ItemCategory::Usable,
            "UsableCustomizationItemDefinition" => ItemCategory::Customization,
            "MissionItemDefinition" => ItemCategory::Mission,
            _ => ItemCategory::Unknown,
        },
        _ => ItemCategory::Unknown,
    }
}

fn tier_of(level: i64, rating: Option<&str>) -> RarityTier {
    // These two rows' ratings do not name their tier, so the label comes from the level. Level
    // 6's row is rated `RARITY_VeryRare`. Level 500's is rated `RARITY_Unknown`; the only
    // definitions that carry 500 are twelve DLC weapon bodies, and every weapon built on one
    // takes its title from a `Title_Pearlescent_…` name part.
    match level {
        6 => return RarityTier::ETech,
        500 => return RarityTier::Pearlescent,
        _ => {}
    }
    match rating {
        Some("RARITY_Common") => RarityTier::Common,
        Some("RARITY_Uncommon") => RarityTier::Uncommon,
        Some("RARITY_Rare") => RarityTier::Rare,
        Some("RARITY_VeryRare") => RarityTier::VeryRare,
        Some("RARITY_Legendary") => RarityTier::Legendary,
        Some("RARITY_Seraph") => RarityTier::Seraph,
        Some("RARITY_Rainbow") => RarityTier::Rainbow,
        _ => RarityTier::Unknown,
    }
}

impl ItemDatabase {
    /// Resolves a saved item under [`ItemDatabase::DLC_RESOLUTION`]: items of a DLC set come
    /// back all-unknown until DLC resolution has been verified in game.
    pub fn info(&self, item: &ItemRefs) -> ItemInfo {
        self.info_with(item, Self::DLC_RESOLUTION)
    }

    /// Resolves a saved item's name, rarity, manufacturer, category and gear class. Anything
    /// that depends on an input that does not resolve is unknown; nothing is guessed.
    ///
    /// With [`DlcResolution::Verified`], items of a DLC set are resolved through the unverified
    /// H-set and H-flag hypotheses; treat such results as provisional.
    pub fn info_with(&self, item: &ItemRefs, dlc: DlcResolution) -> ItemInfo {
        if item.is_virtual() {
            return ItemInfo {
                gear_class: GearClass::NotGear,
                ..ItemInfo::UNKNOWN
            };
        }
        let Some(lookup) = self.lookup(item, dlc) else {
            return ItemInfo::UNKNOWN;
        };
        let type_object = lookup.type_object();
        let category = category_of(type_object);
        ItemInfo {
            name: lookup.name(type_object, category),
            rarity: self.rarity_for_level(rarity_level(&lookup.contributions())),
            manufacturer: lookup.manufacturer(),
            category,
            gear_class: lookup.gear_class(category),
        }
    }

    /// Whether a saved item is gear, under [`ItemDatabase::DLC_RESOLUTION`].
    pub fn gear_class(&self, item: &ItemRefs) -> GearClass {
        self.info(item).gear_class
    }

    /// The rarity contributions of a saved item, in order: its type's base rarity, then each
    /// non-empty part slot's rarity when the category's part slots count
    /// (see [`counts_part_slots`]). Feed them to [`rarity_level`].
    pub fn rarity_contributions(&self, item: &ItemRefs, dlc: DlcResolution) -> Vec<RarityValue> {
        match self.lookup(item, dlc) {
            Some(lookup) => lookup.contributions(),
            None => vec![RarityValue::Unresolvable],
        }
    }

    /// Tier and colour for a rarity level.
    ///
    /// A level with no colour row (e.g. 11), or whose row is exactly `RGBA(0,0,0,0)` (the filler,
    /// 12…170), has no colour, so it renders neutral. Alpha is otherwise not an "unused" marker:
    /// Seraph (501, #FF9AB8) has alpha 0 yet shows pink in game, so such rows are known.
    pub fn rarity_for_level(&self, level: Option<i64>) -> RarityInfo {
        let Some(level) = level else {
            return RarityInfo::UNKNOWN;
        };
        let row = self.color_row(level);
        RarityInfo {
            level: Some(level),
            tier: tier_of(level, row.map(|r| r.rating.as_str())),
            color: row.map(|r| r.color).filter(|&c| c != FILLER_COLOR),
        }
    }

    /// `None` when the item has no set id, or is of a DLC set while those are unverified.
    fn lookup<'a>(&'a self, item: &'a ItemRefs, dlc: DlcResolution) -> Option<Lookup<'a>> {
        let set = item.set?;
        if set != 0 && dlc == DlcResolution::Unverified {
            return None;
        }
        Some(Lookup {
            db: self,
            item,
            set,
            dlc,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{AssetSet, RarityColorRow};

    const EMPTY: u32 = 131071;
    /// The flag bit of a weapon part or item part value (lib bit 5).
    const WEAPON_PART_FLAG: u32 = 32 << 11;
    const ITEM_PART_FLAG: u32 = 32 << 10;
    const ITEM_TYPE_FLAG: u32 = 256 << 8;
    const BALANCE_FLAG: u32 = 512 << 10;

    fn part(name: Option<&str>, rarity: RarityValue) -> ObjectInfo {
        ObjectInfo::Part {
            class_name: "WeaponPartDefinition".into(),
            part_name: name.map(str::to_string),
            rarity,
        }
    }

    fn item_type(class: &str, name: Option<&str>, full: bool, rarity: RarityValue) -> ObjectInfo {
        ObjectInfo::ItemType {
            class_name: class.into(),
            item_name: name.map(str::to_string),
            name_is_full: full,
            base_rarity: rarity,
        }
    }

    fn table(paths: &[&str]) -> Vec<Vec<Option<String>>> {
        vec![paths.iter().map(|p| Some(p.to_string())).collect()]
    }

    /// A tiny database. Base game: weapon type 0 is a pistol with no base rarity, 1 has no known
    /// kind. Weapon parts (lib 0): 0 rarity 2, 1 rarity 4, 2 rarity 5, 3 rarity 6, 4 prefix
    /// "Hot", 5 title "Shot", 6 a part with no name, 7 a part whose rarity is unresolvable.
    /// Item types: 0 shield (base 1), 1 a usable with a full name (502), 2 a customization
    /// (base 2), 3 a mission item, 4 a relic with a full name, 5 an unknown class.
    /// Balance 0 is an ordinary weapon balance, balance 1 a mission weapon's.
    ///
    /// Set 2 (a DLC): weapon parts 0 seraph barrel (497), 1 pearl body (500), 2 title "Pearl";
    /// item type 0 a cross-DLC class mod; item part 0 title "Necromancer"; balance 0 a mission
    /// weapon's.
    ///
    /// The colour rows follow the game table's shape: 0 beige with alpha 0, 6 rated
    /// `RARITY_VeryRare`, no row for 11, 12…170 transparent-black filler.
    fn synthetic() -> ItemDatabase {
        use AssetCategory::*;
        use RarityValue::{None as Absent, Unresolvable, Value};
        let mut base = AssetSet::new(None);
        base.set_tables(WeaponTypes, table(&["T.Pistol", "T.Odd"]));
        base.set_tables(
            WeaponParts,
            table(&[
                "P.R2", "P.R4", "P.R5", "P.R6", "P.Prefix", "P.Title", "P.Body", "P.Unres",
            ]),
        );
        base.set_tables(
            ItemTypes,
            table(&[
                "I.Shield",
                "I.Echo",
                "I.Skin",
                "I.Mission",
                "I.Relic",
                "I.Pool",
            ]),
        );
        base.set_tables(ItemParts, table(&["IP.Material", "IP.Title", "IP.Prefix"]));
        base.set_tables(Manufacturers, table(&["M.Maker", "M.Nameless"]));
        base.set_tables(Balances, table(&["B.Weapon", "B.Mission"]));
        let mut dlc = AssetSet::new(Some("Tulip".into()));
        dlc.set_tables(WeaponParts, table(&["D.Seraph", "D.Pearl", "D.Title"]));
        dlc.set_tables(ItemTypes, table(&["D.ClassMod"]));
        dlc.set_tables(ItemParts, table(&["D.ModTitle"]));
        dlc.set_tables(Balances, table(&["D.Mission"]));

        let mut db = ItemDatabase::new("synthetic");
        db.sets.insert(0, base);
        db.sets.insert(2, dlc);
        let objects = [
            (
                "T.Pistol",
                ObjectInfo::WeaponType {
                    kind: Some(WeaponKind::Pistol),
                    type_name: Some("Pistol".into()),
                    base_rarity: Absent,
                },
            ),
            (
                "T.Odd",
                ObjectInfo::WeaponType {
                    kind: None,
                    type_name: None,
                    base_rarity: Absent,
                },
            ),
            ("P.R2", part(None, Value(2.0))),
            ("P.R4", part(None, Value(4.0))),
            ("P.R5", part(None, Value(5.0))),
            ("P.R6", part(None, Value(6.0))),
            ("P.Prefix", part(Some("Hot"), Absent)),
            ("P.Title", part(Some("Shot"), Absent)),
            ("P.Body", part(None, Absent)),
            ("P.Unres", part(None, Unresolvable)),
            (
                "I.Shield",
                item_type("ShieldDefinition", Some("Shield"), false, Value(1.0)),
            ),
            (
                "I.Echo",
                item_type(
                    "UsableItemDefinition",
                    Some("Helmet Data"),
                    true,
                    Value(502.0),
                ),
            ),
            (
                "I.Skin",
                item_type("UsableCustomizationItemDefinition", None, false, Value(2.0)),
            ),
            (
                "I.Mission",
                item_type("MissionItemDefinition", Some("Parts"), false, Value(10.0)),
            ),
            (
                "I.Relic",
                item_type("ArtifactDefinition", Some("Vault Relic"), true, Absent),
            ),
            (
                "I.Pool",
                item_type("ItemPoolDefinition", Some("Pool"), true, Absent),
            ),
            ("IP.Material", part(None, Value(3.0))),
            ("IP.Title", part(Some("Turtle Shield"), Absent)),
            ("IP.Prefix", part(Some("Grounded"), Absent)),
            (
                "M.Maker",
                ObjectInfo::Manufacturer {
                    display_name: Some("Maker".into()),
                },
            ),
            (
                "M.Nameless",
                ObjectInfo::Manufacturer { display_name: None },
            ),
            (
                "B.Weapon",
                ObjectInfo::Balance {
                    class_name: "WeaponBalanceDefinition".into(),
                },
            ),
            (
                "B.Mission",
                ObjectInfo::Balance {
                    class_name: "MissionWeaponBalanceDefinition".into(),
                },
            ),
            ("D.Seraph", part(None, Value(497.0))),
            ("D.Pearl", part(None, Value(500.0))),
            ("D.Title", part(Some("Pearl"), Absent)),
            (
                "D.ClassMod",
                item_type("CrossDLCClassModDefinition", None, false, Absent),
            ),
            ("D.ModTitle", part(Some("Necromancer"), Absent)),
            (
                "D.Mission",
                ObjectInfo::Balance {
                    class_name: "MissionWeaponBalanceDefinition".into(),
                },
            ),
        ];
        db.objects
            .extend(objects.map(|(path, info)| (path.to_string(), info)));
        let row = |min, max, color: Rgba, rating: &str| RarityColorRow {
            min_level: min,
            max_level: max,
            color,
            rating: rating.into(),
        };
        db.rarity_colors = vec![
            row(0, 0, Rgba::new(205, 193, 175, 0), "RARITY_Unknown"),
            row(1, 1, Rgba::new(255, 255, 255, 255), "RARITY_Common"),
            row(2, 2, Rgba::new(61, 210, 11, 255), "RARITY_Uncommon"),
            row(3, 3, Rgba::new(60, 142, 255, 255), "RARITY_Rare"),
            row(4, 4, Rgba::new(168, 63, 229, 255), "RARITY_VeryRare"),
            row(5, 5, Rgba::new(255, 180, 0, 255), "RARITY_Legendary"),
            row(6, 6, Rgba::new(202, 0, 168, 255), "RARITY_VeryRare"),
            row(7, 10, Rgba::new(255, 180, 0, 255), "RARITY_Legendary"),
            row(12, 170, Rgba::new(0, 0, 0, 0), "RARITY_Unknown"),
            row(500, 500, Rgba::new(0, 255, 255, 0), "RARITY_Unknown"),
            row(501, 501, Rgba::new(255, 154, 184, 0), "RARITY_Seraph"),
            row(502, 502, Rgba::new(255, 255, 255, 0), "RARITY_Unknown"),
            row(506, 506, Rgba::new(242, 255, 161, 255), "RARITY_Rainbow"),
        ];
        db
    }

    /// A base-game pistol: `slots` gives part-slot index and weapon-part value; other slots are
    /// empty.
    fn pistol(slots: &[(usize, u32)]) -> ItemRefs {
        let mut parts = [Some(EMPTY); PART_SLOTS];
        for &(index, value) in slots {
            parts[index] = Some(value);
        }
        ItemRefs {
            is_weapon: true,
            set: Some(0),
            item_type: Some(0),
            balance: Some(0),
            manufacturer: Some(0),
            parts,
        }
    }

    /// A base-game item of type `item_type` whose part slots are all empty.
    fn item(item_type: u32) -> ItemRefs {
        ItemRefs {
            is_weapon: false,
            set: Some(0),
            item_type: Some(item_type),
            balance: Some(0),
            manufacturer: Some(2047),
            parts: [Some(65535); PART_SLOTS],
        }
    }

    fn rarity(level: i64, tier: RarityTier, color: Option<Rgba>) -> RarityInfo {
        RarityInfo {
            level: Some(level),
            tier,
            color,
        }
    }

    #[test]
    fn prefix_title_name_and_summed_legendary() {
        let db = synthetic();
        let info = db.info(&pistol(&[(0, 1), (2, 2), (9, 4), (10, 5)]));
        assert_eq!(
            info,
            ItemInfo {
                name: Some("Hot Shot".into()),
                rarity: rarity(9, RarityTier::Legendary, Some(Rgba::new(255, 180, 0, 255))),
                manufacturer: Some("Maker".into()),
                category: ItemCategory::Weapon(WeaponKind::Pistol),
                gear_class: GearClass::Gear,
            }
        );
        assert_eq!(
            db.rarity_contributions(
                &pistol(&[(0, 1), (2, 2), (9, 4), (10, 5)]),
                DlcResolution::Unverified
            ),
            [
                RarityValue::None,
                RarityValue::Value(4.0),
                RarityValue::Value(5.0),
                RarityValue::None,
                RarityValue::None
            ]
        );
        // No prefix: the title alone.
        assert_eq!(db.info(&pistol(&[(10, 5)])).name.as_deref(), Some("Shot"));
        // No title: unknown, even with a prefix.
        assert_eq!(db.info(&pistol(&[(9, 4)])).name, None);
        // A name slot that holds a part without a name, or nothing in the database.
        assert_eq!(db.info(&pistol(&[(9, 6), (10, 5)])).name, None);
        assert_eq!(db.info(&pistol(&[(9, 4), (10, 6)])).name, None);
        assert_eq!(db.info(&pistol(&[(9, 900), (10, 5)])).name, None);
        // Absent slots (a short bitstream) are empty too.
        let mut short = pistol(&[(10, 5)]);
        short.parts[..10].fill(None);
        assert_eq!(db.info(&short).name.as_deref(), Some("Shot"));
        assert_eq!(db.info(&short).rarity.level, Some(0));
    }

    /// Ledger ruling (c): row 6 is rated `RARITY_VeryRare`; the e-tech label comes from the level.
    #[test]
    fn e_tech_tier_comes_from_level_six() {
        let db = synthetic();
        let r = db.info(&pistol(&[(0, 1), (2, 0)])).rarity; // 4 + 2
        assert_eq!(
            r,
            rarity(6, RarityTier::ETech, Some(Rgba::new(202, 0, 168, 255)))
        );
        // Level 4 shares the rating but stays very rare.
        assert_eq!(
            db.info(&pistol(&[(0, 1)])).rarity.tier,
            RarityTier::VeryRare
        );
    }

    /// Ledger ruling (b): a level with no colour row (11) renders neutral, never black.
    #[test]
    fn level_eleven_has_no_colour() {
        let r = synthetic().info(&pistol(&[(0, 3), (2, 2)])).rarity; // 6 + 5
        assert_eq!(r, rarity(11, RarityTier::Unknown, None));
    }

    /// Ledger ruling (b, amended): only the transparent-black filler row (12…170) renders
    /// neutral. Other rows with alpha 0 (here 0, beige) keep their colour, alpha as stored.
    #[test]
    fn filler_row_has_no_colour() {
        let db = synthetic();
        let twelve = db.info(&pistol(&[(0, 3), (2, 3)])).rarity; // 6 + 6
        assert_eq!(twelve, rarity(12, RarityTier::Unknown, None));
        let zero = db.info(&pistol(&[])).rarity;
        assert_eq!(
            zero,
            rarity(0, RarityTier::Unknown, Some(Rgba::new(205, 193, 175, 0)))
        );
    }

    #[test]
    fn tiers_and_colours_by_level() {
        let db = synthetic();
        let tier = |level| db.rarity_for_level(Some(level)).tier;
        assert_eq!(
            [1, 2, 3, 4, 5, 6, 7, 10].map(tier),
            [
                RarityTier::Common,
                RarityTier::Uncommon,
                RarityTier::Rare,
                RarityTier::VeryRare,
                RarityTier::Legendary,
                RarityTier::ETech,
                RarityTier::Legendary,
                RarityTier::Legendary
            ]
        );
        // Seraph's row has alpha 0 but is a real colour.
        assert_eq!(
            db.rarity_for_level(Some(501)),
            rarity(501, RarityTier::Seraph, Some(Rgba::new(255, 154, 184, 0)))
        );
        // Pearlescent's row is rated unknown; the label comes from the level.
        assert_eq!(
            db.rarity_for_level(Some(500)),
            rarity(
                500,
                RarityTier::Pearlescent,
                Some(Rgba::new(0, 255, 255, 0))
            )
        );
        assert_eq!(tier(506), RarityTier::Rainbow);
        assert_eq!(db.rarity_for_level(Some(502)).tier, RarityTier::Unknown);
        assert_eq!(
            db.rarity_for_level(Some(170)),
            rarity(170, RarityTier::Unknown, None)
        );
        assert_eq!(
            db.rarity_for_level(Some(-3)),
            rarity(-3, RarityTier::Unknown, None)
        );
        assert_eq!(db.rarity_for_level(None), RarityInfo::UNKNOWN);
    }

    #[test]
    fn unresolved_parts_make_rarity_unknown() {
        let db = synthetic();
        // A slot outside the tables, and a part whose own rarity is unresolvable.
        for slots in [
            [(0, 1), (2, 900)],
            [(0, 1), (2, 7)],
            [(0, 1), (2, (63 << 11) | 5)],
        ] {
            let info = db.info(&pistol(&slots));
            assert_eq!(info.rarity, RarityInfo::UNKNOWN, "{slots:?}");
            // The rest of the item is still known.
            assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));
            assert_eq!(info.manufacturer.as_deref(), Some("Maker"));
            assert_eq!(info.gear_class, GearClass::Gear);
        }
    }

    #[test]
    fn unresolved_type_makes_the_item_unknown() {
        let db = synthetic();
        let unknown_type = |refs: ItemRefs| {
            let info = db.info(&refs);
            assert_eq!(info.category, ItemCategory::Unknown);
            assert_eq!(info.gear_class, GearClass::Unknown);
            assert_eq!(info.rarity, RarityInfo::UNKNOWN);
            assert_eq!(info.name, None);
            info
        };
        let mut refs = pistol(&[(0, 1), (10, 5)]);
        refs.item_type = Some((127 << 6) | 63);
        // The manufacturer does not depend on the type.
        assert_eq!(
            unknown_type(refs.clone()).manufacturer.as_deref(),
            Some("Maker")
        );
        refs.item_type = None;
        unknown_type(refs.clone());
        // A weapon type without a known kind.
        refs.item_type = Some(1);
        unknown_type(refs.clone());
        // An item read as a weapon: its type value lands on a weapon type, or on nothing.
        let mut crossed = item(0);
        crossed.is_weapon = true;
        assert_eq!(
            db.info(&crossed).category,
            ItemCategory::Weapon(WeaponKind::Pistol)
        );
        let mut crossed = pistol(&[]);
        crossed.is_weapon = false;
        crossed.item_type = Some(900);
        assert_eq!(db.info(&crossed).category, ItemCategory::Unknown);
    }

    #[test]
    fn manufacturer_resolution() {
        let db = synthetic();
        let with = |manufacturer| {
            let mut refs = pistol(&[]);
            refs.manufacturer = manufacturer;
            db.info(&refs).manufacturer
        };
        assert_eq!(with(Some(0)).as_deref(), Some("Maker"));
        assert_eq!(with(Some(1)), None); // no display name
        assert_eq!(with(Some(2047)), None); // all ones
        assert_eq!(with(None), None);
    }

    /// Mission weapons are not gear, but their part slots still hold real weapon parts.
    #[test]
    fn mission_weapon_is_not_gear() {
        let db = synthetic();
        let mut refs = pistol(&[(0, 1), (9, 4), (10, 5)]);
        refs.balance = Some(1);
        let info = db.info(&refs);
        assert_eq!(info.gear_class, GearClass::NotGear);
        assert_eq!(db.gear_class(&refs), GearClass::NotGear);
        assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));
        assert_eq!(info.rarity.level, Some(4));
        assert_eq!(info.name.as_deref(), Some("Hot Shot"));
        // A balance that does not resolve does not change the class.
        for balance in [Some(900), None] {
            refs.balance = balance;
            assert_eq!(db.gear_class(&refs), GearClass::Gear);
        }
    }

    #[test]
    fn item_categories_names_and_gear() {
        let db = synthetic();
        // A shield: name from prefix and title, rarity from base plus material.
        let mut shield = item(0);
        shield.parts[8] = Some(0);
        shield.parts[9] = Some(2);
        shield.parts[10] = Some(1);
        let info = db.info(&shield);
        assert_eq!(info.name.as_deref(), Some("Grounded Turtle Shield"));
        assert_eq!(info.rarity.level, Some(4)); // 1 + 3
        assert_eq!(info.category, ItemCategory::Shield);
        assert_eq!(info.gear_class, GearClass::Gear);
        assert_eq!(info.manufacturer, None);
        // Without a title its `ItemName` is not used: that is not the full name.
        assert_eq!(db.info(&item(0)).name, None);

        // A usable item with a full name: its part slots are not looked at.
        let mut echo = item(1);
        echo.parts = [None; PART_SLOTS];
        let info = db.info(&echo);
        assert_eq!(info.name.as_deref(), Some("Helmet Data"));
        assert_eq!(
            info.rarity,
            rarity(502, RarityTier::Unknown, Some(Rgba::new(255, 255, 255, 0)))
        );
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::Usable, GearClass::NotGear)
        );

        // A customization: garbage in the part slots is ignored, and it has no name of its own.
        let mut skin = item(2);
        skin.parts = [Some(40000); PART_SLOTS];
        let info = db.info(&skin);
        assert_eq!(info.name, None);
        assert_eq!(info.rarity.level, Some(2));
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::Customization, GearClass::NotGear)
        );

        // A mission item uses its `ItemName` even though it is not flagged as the full name.
        let info = db.info(&item(3));
        assert_eq!(info.name.as_deref(), Some("Parts"));
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::Mission, GearClass::NotGear)
        );
        assert_eq!(
            info.rarity,
            rarity(10, RarityTier::Legendary, Some(Rgba::new(255, 180, 0, 255)))
        );

        // A relic with a full name keeps it whatever the name slots hold.
        let mut relic = item(4);
        relic.parts[10] = Some(1);
        let info = db.info(&relic);
        assert_eq!(info.name.as_deref(), Some("Vault Relic"));
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::Relic, GearClass::Gear)
        );

        // A class this crate does not know: named (the name is flagged full) but unclassified.
        let info = db.info(&item(5));
        assert_eq!(info.name.as_deref(), Some("Pool"));
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::Unknown, GearClass::Unknown)
        );
        assert_eq!(info.rarity, RarityInfo::UNKNOWN);
    }

    #[test]
    fn virtual_and_unset_items() {
        let db = synthetic();
        let virtual_item = ItemRefs {
            is_weapon: true,
            set: Some(255),
            item_type: Some(0),
            balance: Some(0),
            manufacturer: Some(0),
            parts: [Some(0); PART_SLOTS],
        };
        assert!(virtual_item.is_virtual());
        assert_eq!(
            db.info(&virtual_item),
            ItemInfo {
                gear_class: GearClass::NotGear,
                ..ItemInfo::UNKNOWN
            }
        );
        assert_eq!(db.gear_class(&virtual_item), GearClass::NotGear);
        let mut absent = virtual_item.clone();
        absent.balance = None;
        absent.parts = [None; PART_SLOTS];
        assert!(absent.is_virtual());

        // Set 255 with any reference in it is not the placeholder, and is not resolved.
        let mut odd = virtual_item.clone();
        odd.parts[3] = Some(7);
        assert!(!odd.is_virtual());
        assert_eq!(db.info(&odd), ItemInfo::UNKNOWN);
        assert_eq!(
            db.info_with(&odd, DlcResolution::Verified).gear_class,
            GearClass::Gear
        );

        let mut no_set = pistol(&[(10, 5)]);
        no_set.set = None;
        assert!(!no_set.is_virtual());
        assert_eq!(db.info(&no_set), ItemInfo::UNKNOWN);
        assert_eq!(
            db.info_with(&no_set, DlcResolution::Verified),
            ItemInfo::UNKNOWN
        );
        assert_eq!(
            db.rarity_contributions(&no_set, DlcResolution::Verified),
            [RarityValue::Unresolvable]
        );
    }

    /// The plan's policy: items of a DLC set stay all-unknown, and so never duplicable, until
    /// DLC resolution has been verified in game.
    #[test]
    fn dlc_items_are_unknown_while_unverified() {
        let db = synthetic();
        let mut refs = pistol(&[(0, 1), (9, 4), (10, 5)]);
        refs.set = Some(2);
        assert_eq!(db.info(&refs), ItemInfo::UNKNOWN);
        assert_eq!(db.gear_class(&refs), GearClass::Unknown);
        assert_eq!(
            db.info_with(&refs, DlcResolution::Unverified),
            ItemInfo::UNKNOWN
        );
        assert_eq!(
            db.rarity_contributions(&refs, DlcResolution::Unverified),
            [RarityValue::Unresolvable]
        );
    }

    #[test]
    fn dlc_items_resolve_when_asked_to() {
        let db = synthetic();
        let verified = DlcResolution::Verified;

        // A seraph weapon: a base pistol type and body (4), a DLC barrel (497) and title.
        let mut seraph = pistol(&[
            (0, 1),
            (2, WEAPON_PART_FLAG),
            (9, 4),
            (10, WEAPON_PART_FLAG | 2),
        ]);
        seraph.set = Some(2);
        let info = db.info_with(&seraph, verified);
        assert_eq!(info.name.as_deref(), Some("Hot Pearl"));
        assert_eq!(
            info.rarity,
            rarity(501, RarityTier::Seraph, Some(Rgba::new(255, 154, 184, 0)))
        );
        assert_eq!(info.manufacturer.as_deref(), Some("Maker"));
        assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));
        assert_eq!(info.gear_class, GearClass::Gear);

        // A pearlescent: the DLC body alone carries 500.
        let mut pearl = pistol(&[(0, WEAPON_PART_FLAG | 1), (10, WEAPON_PART_FLAG | 2)]);
        pearl.set = Some(2);
        let info = db.info_with(&pearl, verified);
        assert_eq!(info.name.as_deref(), Some("Pearl"));
        assert_eq!(
            info.rarity,
            rarity(
                500,
                RarityTier::Pearlescent,
                Some(Rgba::new(0, 255, 255, 0))
            )
        );

        // A DLC mission weapon balance still means not gear.
        pearl.balance = Some(BALANCE_FLAG);
        assert_eq!(
            db.info_with(&pearl, verified).gear_class,
            GearClass::NotGear
        );

        // A cross-DLC class mod: DLC type and title, no manufacturer.
        let mut class_mod = item(ITEM_TYPE_FLAG);
        class_mod.set = Some(2);
        class_mod.parts[10] = Some(ITEM_PART_FLAG);
        let info = db.info_with(&class_mod, verified);
        assert_eq!(info.name.as_deref(), Some("Necromancer"));
        assert_eq!(info.rarity.level, Some(0));
        assert_eq!(
            (info.category, info.gear_class),
            (ItemCategory::ClassMod, GearClass::Gear)
        );

        // A flagged reference the set does not have, and a set the database does not have.
        let mut missing = pistol(&[(0, 1), (2, WEAPON_PART_FLAG | 9), (10, 5)]);
        missing.set = Some(2);
        let info = db.info_with(&missing, verified);
        assert_eq!(info.rarity, RarityInfo::UNKNOWN);
        assert_eq!(info.name.as_deref(), Some("Shot"));
        let mut other_set = seraph.clone();
        other_set.set = Some(9);
        let info = db.info_with(&other_set, verified);
        assert_eq!((info.name, info.rarity), (None, RarityInfo::UNKNOWN));
        assert_eq!(info.category, ItemCategory::Weapon(WeaponKind::Pistol));

        // The same numbers in a base-game item never reach the DLC set.
        seraph.set = Some(0);
        let info = db.info_with(&seraph, verified);
        assert_eq!((info.name, info.rarity), (None, RarityInfo::UNKNOWN));
    }
}
