//! The item catalog: what the game's own data says about a saved item.
//!
//! Wraps [`gamedata::ItemDatabase`] for the rest of the app: it turns a decoded item into a
//! display name, rarity, manufacturer and category, and it is the [`GearClassifier`] that
//! decides what Duplicate Item is offered on.
//!
//! Items from DLC packs resolve through tables that have not yet been compared with the game
//! (see `docs/research/2026-11-09-windows-game-data.md`). For those the catalog gives its best
//! reading marked `unverified`, and classifies them as unknown, so they are shown but are not
//! offered for duplication until the reading has been confirmed in game.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use gamedata::{
    AssetCategory, CardLineKind, DatabaseCache, DlcResolution, GameInstall, ImportProgress, ItemCategory, ItemDatabase,
    ItemInfo, ItemLevel, ItemRefs, RarityTier, StatKind, StatValue, WeaponKind,
};
use savekit::{GearClass, GearClassifier, ItemFields};

/// What to show for one item. Every field is `None` (or `"unknown"`) when the game data has
/// no answer; nothing here is ever guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDisplay {
    /// The name the game shows, such as "Gearbox Rifle".
    pub name: Option<String>,
    /// Rarity tier key: `common`, `uncommon`, `rare`, `very-rare`, `e-tech`, `legendary`,
    /// `seraph`, `pearlescent`, `rainbow` or `unknown`.
    pub tier: &'static str,
    /// Rarity tier as shown: "Legendary".
    pub tier_label: &'static str,
    /// The game's own color for that rarity, as `#RRGGBB`.
    pub color: Option<String>,
    pub manufacturer: Option<String>,
    /// Category key: `pistol`, `shotgun`, `smg`, `assault-rifle`, `sniper-rifle`, `launcher`,
    /// `shield`, `grenade-mod`, `class-mod`, `relic`, `usable`, `customization`, `mission`
    /// or `unknown`.
    pub category: &'static str,
    /// Category as shown: "Assault Rifle".
    pub category_label: &'static str,
    /// True when this came from DLC tables that have not been confirmed in game.
    pub unverified: bool,
    /// The name of the item's type definition in the game's files, such as
    /// `Head_MechromancerAster`. Something to show for an item that has no display name.
    /// For DLC items it comes from the same unconfirmed tables as `unverified` readings.
    pub definition: Option<String>,
}

impl ItemDisplay {
    pub const UNKNOWN: ItemDisplay = ItemDisplay {
        name: None,
        tier: "unknown",
        tier_label: "Unknown",
        color: None,
        manufacturer: None,
        category: "unknown",
        category_label: "Item",
        unverified: false,
        definition: None,
    };
}

/// One row of an item's card, such as "Damage 13".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatLine {
    /// Which row this is: `damage`, `accuracy`, `fire-rate`, `reload-speed`, `magazine-size`,
    /// `elemental-damage`, `elemental-chance`, `shield-capacity`, `shield-recharge-rate`,
    /// `shield-recharge-delay` or `other`.
    pub key: &'static str,
    /// The row's label from the game's data, such as "Damage".
    pub label: String,
    /// The value as the card prints it, such as "13" or "37 x7". `None` when the row exists
    /// on the card but could not be worked out; nothing is guessed.
    pub text: Option<String>,
    /// Why the value may differ from the game's card, or why there is none.
    pub note: Option<String>,
}

/// A line of the card below its rows: fixed text (a unique weapon's red text) or a bonus the
/// card composes from a number ("+8% Critical Hit Damage").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLine {
    /// `text` for fixed text, `bonus` for a composed line.
    pub kind: &'static str,
    pub text: String,
    /// The game's color for the line, as `#RRGGBB`.
    pub color: Option<String>,
    /// Why the line may differ from the game's card, if it may.
    pub note: Option<String>,
}

/// An item's card as computed from the game's data: the rows the game shows under the item's
/// name. **Computed, not yet compared with the game** (see
/// `docs/research/2026-11-09-item-stats.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCard {
    pub rows: Vec<StatLine>,
    /// The lines below the rows, in the card's order. Lines the game shows but that could not
    /// be composed are left out, and counted in `missing_lines`.
    pub lines: Vec<CardLine>,
    pub missing_lines: usize,
}

fn stat_key(kind: StatKind) -> &'static str {
    match kind {
        StatKind::Damage => "damage",
        StatKind::Accuracy => "accuracy",
        StatKind::FireRate => "fire-rate",
        StatKind::ReloadSpeed => "reload-speed",
        StatKind::MagazineSize => "magazine-size",
        StatKind::ElementalDamage => "elemental-damage",
        StatKind::ElementalChance => "elemental-chance",
        StatKind::ShieldCapacity => "shield-capacity",
        StatKind::ShieldRechargeRate => "shield-recharge-rate",
        StatKind::ShieldRechargeDelay => "shield-recharge-delay",
        StatKind::Other => "other",
    }
}

fn tier_names(tier: RarityTier) -> (&'static str, &'static str) {
    match tier {
        RarityTier::Common => ("common", "Common"),
        RarityTier::Uncommon => ("uncommon", "Uncommon"),
        RarityTier::Rare => ("rare", "Rare"),
        RarityTier::VeryRare => ("very-rare", "Very Rare"),
        RarityTier::ETech => ("e-tech", "E-tech"),
        RarityTier::Legendary => ("legendary", "Legendary"),
        RarityTier::Seraph => ("seraph", "Seraph"),
        RarityTier::Pearlescent => ("pearlescent", "Pearlescent"),
        RarityTier::Rainbow => ("rainbow", "Rainbow"),
        RarityTier::Unknown => ("unknown", "Unknown"),
    }
}

fn category_names(category: ItemCategory) -> (&'static str, &'static str) {
    match category {
        ItemCategory::Weapon(WeaponKind::Pistol) => ("pistol", "Pistol"),
        ItemCategory::Weapon(WeaponKind::Shotgun) => ("shotgun", "Shotgun"),
        ItemCategory::Weapon(WeaponKind::Smg) => ("smg", "SMG"),
        ItemCategory::Weapon(WeaponKind::AssaultRifle) => ("assault-rifle", "Assault Rifle"),
        ItemCategory::Weapon(WeaponKind::SniperRifle) => ("sniper-rifle", "Sniper Rifle"),
        ItemCategory::Weapon(WeaponKind::Launcher) => ("launcher", "Rocket Launcher"),
        ItemCategory::Shield => ("shield", "Shield"),
        ItemCategory::GrenadeMod => ("grenade-mod", "Grenade Mod"),
        ItemCategory::ClassMod => ("class-mod", "Class Mod"),
        ItemCategory::Relic => ("relic", "Relic"),
        ItemCategory::Usable => ("usable", "Usable"),
        ItemCategory::Customization => ("customization", "Customization"),
        ItemCategory::Mission => ("mission", "Mission Item"),
        ItemCategory::Unknown => ("unknown", "Item"),
    }
}

/// The numbers `gamedata` looks an item up by.
pub fn item_refs(fields: &ItemFields, is_weapon: bool) -> ItemRefs {
    ItemRefs {
        is_weapon,
        set: fields.set,
        item_type: fields.item_type,
        balance: fields.balance,
        manufacturer: fields.manufacturer,
        parts: fields.parts,
    }
}

fn is_blank(info: &ItemInfo) -> bool {
    info.name.is_none()
        && info.manufacturer.is_none()
        && info.category == ItemCategory::Unknown
        && info.rarity.tier == RarityTier::Unknown
}

/// Item names, rarity and gear classes, from the user's own game install.
pub struct Catalog {
    db: ItemDatabase,
}

impl Catalog {
    pub fn new(db: ItemDatabase) -> Catalog {
        Catalog { db }
    }

    /// Read the game data: from the local cache when it matches the installed game, otherwise
    /// from the game's files (read-only), which takes a few seconds and refreshes the cache.
    ///
    /// `cache_dir` overrides where the cache lives (default `%LOCALAPPDATA%\VaultKey\GameData`).
    /// The error is a message for the user; the app works without a catalog.
    pub fn load(cache_dir: Option<PathBuf>, progress: &mut dyn FnMut(ImportProgress)) -> Result<Catalog, String> {
        let install = GameInstall::locate().ok_or_else(|| {
            "Borderlands 2 was not found in your Steam libraries. Item names and pictures need the game installed on this PC.".to_string()
        })?;
        let dir = cache_dir
            .or_else(DatabaseCache::default_directory)
            .ok_or_else(|| "There is no local app-data folder to keep the item data in.".to_string())?;
        let cache = DatabaseCache::new(dir, &install).map_err(|e| e.to_string())?;
        let db = gamedata::load_or_import(&install, &cache, progress, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
        Ok(Catalog { db })
    }

    pub fn database(&self) -> &ItemDatabase {
        &self.db
    }

    /// What to show for an item.
    pub fn describe(&self, fields: &ItemFields, is_weapon: bool) -> ItemDisplay {
        let refs = item_refs(fields, is_weapon);
        let mut info = self.db.info(&refs);
        let mut unverified = false;
        if is_blank(&info) && fields.set.is_some_and(|set| set != 0) && !refs.is_virtual() {
            let provisional = self.db.info_with(&refs, DlcResolution::Verified);
            if !is_blank(&provisional) {
                info = provisional;
                unverified = true;
            }
        }
        let definition = match (fields.item_type, fields.set) {
            (Some(value), Some(set)) if !refs.is_virtual() => {
                let types = if is_weapon { AssetCategory::WeaponTypes } else { AssetCategory::ItemTypes };
                self.db
                    .path_with(types, value, set, DlcResolution::Verified)
                    .and_then(|path| path.rsplit('.').next())
                    .map(str::to_string)
            }
            _ => None,
        };
        let (tier, tier_label) = tier_names(info.rarity.tier);
        let (category, category_label) = category_names(info.category);
        ItemDisplay {
            name: info.name,
            tier,
            tier_label,
            color: info.rarity.color.map(|c| c.hex()),
            manufacturer: info.manufacturer,
            category,
            category_label,
            unverified,
            definition,
        }
    }
}

impl Catalog {
    /// The item's card rows, computed from the game's data at the item's own level. `None`
    /// when the game data cannot produce a card for this item (a kind that is not computed
    /// yet, or an item that does not resolve).
    pub fn card(&self, fields: &ItemFields, is_weapon: bool) -> Option<ItemCard> {
        let refs = item_refs(fields, is_weapon);
        let level = ItemLevel { grade_index: fields.grade_index, game_stage: fields.game_stage };
        let stats = self.db.stats(&refs, level);
        if stats.unavailable.is_some() || (stats.rows.is_empty() && stats.lines.is_empty()) {
            return None;
        }
        let rows = stats
            .rows
            .iter()
            .map(|row| {
                let note = match &row.value {
                    StatValue::Computed { approximate, .. } => approximate.clone(),
                    StatValue::Unknown { reason } => Some(reason.clone()),
                };
                StatLine {
                    key: stat_key(row.kind()),
                    label: row.label.clone().unwrap_or_else(|| "Stat".to_string()),
                    text: row.text(),
                    note,
                }
            })
            .collect();
        let lines: Vec<CardLine> = stats
            .lines
            .iter()
            .filter_map(|line| {
                Some(CardLine {
                    kind: match line.kind {
                        CardLineKind::Text => "text",
                        CardLineKind::Bonus => "bonus",
                    },
                    text: line.text.clone()?,
                    color: line.color.map(|c| c.hex()),
                    note: line.note.clone(),
                })
            })
            .collect();
        let missing_lines = stats.lines.iter().filter(|line| line.text.is_none()).count();
        Some(ItemCard { rows, lines, missing_lines })
    }
}

impl GearClassifier for Catalog {
    fn gear_class(&self, fields: &ItemFields, is_weapon: bool) -> GearClass {
        match self.db.gear_class(&item_refs(fields, is_weapon)) {
            gamedata::GearClass::Gear => GearClass::Gear,
            gamedata::GearClass::NotGear => GearClass::NotGear,
            gamedata::GearClass::Unknown => GearClass::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savekit::{ItemBlob, SaveDocument};
    use std::path::Path;

    fn live_catalog() -> Option<Catalog> {
        // A temp cache folder: tests never write to the user's real app-data folder.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("gk-catalog-test-{}-{n}", std::process::id()));
        let catalog = Catalog::load(Some(dir.clone()), &mut |_| {}).ok();
        let _ = std::fs::remove_dir_all(dir);
        if catalog.is_none() {
            eprintln!("skipped: no game install found");
        }
        catalog
    }

    #[test]
    fn empty_database_knows_nothing_and_duplicates_nothing() {
        let catalog = Catalog::new(ItemDatabase::new("test"));
        // The Infinity pistol from the format doc's test vectors.
        let blob = ItemBlob::from_code("BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)").unwrap();
        let fields = ItemFields::from_blob(&blob);
        assert_eq!(catalog.describe(&fields, true), ItemDisplay::UNKNOWN);
        assert_eq!(catalog.gear_class(&fields, true), GearClass::Unknown);
    }

    #[test]
    fn real_game_data_names_the_spec_vectors() {
        let Some(catalog) = live_catalog() else { return };
        let pistol = ItemBlob::from_code("BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)").unwrap();
        let d = catalog.describe(&ItemFields::from_blob(&pistol), true);
        assert!(d.name.as_deref().is_some_and(|n| n.ends_with("Infinity")), "{d:?}");
        assert_eq!((d.tier, d.category, d.manufacturer.as_deref()), ("legendary", "pistol", Some("Vladof")));
        assert_eq!(d.color.as_deref(), Some("#FFB400"));
        assert!(!d.unverified);
        assert_eq!(catalog.gear_class(&ItemFields::from_blob(&pistol), true), GearClass::Gear);

        // The Infinity's card: the five rows every weapon has, all with a value.
        let card = catalog.card(&ItemFields::from_blob(&pistol), true).expect("a weapon has a card");
        let keys: Vec<_> = card.rows.iter().map(|row| row.key).collect();
        assert_eq!(&keys[..5], ["damage", "accuracy", "fire-rate", "reload-speed", "magazine-size"], "{card:?}");
        assert!(card.rows.iter().all(|row| row.text.is_some() && !row.label.is_empty()), "{card:?}");

        let relic = ItemBlob::from_code("BL2(BwAAAABrxwAYC17YIBGBAABAAMD/////////Pw2AAwA=)").unwrap();
        let d = catalog.describe(&ItemFields::from_blob(&relic), false);
        assert_eq!((d.name.as_deref(), d.category), (Some("Vault Hunter's Relic"), "relic"));
        // The relic's card: a line of text, and nothing made up for its unknown row.
        let card = catalog.card(&ItemFields::from_blob(&relic), false).expect("a relic has a card");
        assert!(card.lines.iter().any(|l| l.kind == "text" && !l.text.is_empty()), "{card:?}");
        assert!(card.rows.iter().all(|row| row.text.is_some() || row.note.is_some()), "{card:?}");
    }

    #[test]
    fn fixture_items_are_described_and_only_gear_duplicates() {
        let Some(catalog) = live_catalog() else { return };
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local");
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "sav")) {
            let doc = SaveDocument::from_file_bytes(&std::fs::read(&path).unwrap()).unwrap();
            for entry in doc.inventory() {
                let Some(fields) = &entry.fields else { continue };
                let d = catalog.describe(fields, entry.is_weapon);
                let duplicable = entry.is_duplicable(&catalog);
                if entry.is_virtual {
                    assert!(!duplicable && d.name.is_none() && d.definition.is_none(), "{path:?} {}", entry.entry_ref);
                    continue;
                }
                // Whatever has no display name still says which game definition it is.
                assert!(d.name.is_some() || d.definition.is_some(), "{path:?} {} {d:?}", entry.entry_ref);
                // Every weapon in a real save is gear with a name; nothing unverified or
                // non-gear is ever offered for duplication.
                if entry.is_weapon && fields.set == Some(0) {
                    assert!(d.name.is_some() && duplicable, "{path:?} {} {d:?}", entry.entry_ref);
                }
                if d.unverified || matches!(d.category, "customization" | "mission" | "usable" | "unknown") {
                    assert!(!duplicable, "{path:?} {} {d:?}", entry.entry_ref);
                }
            }
        }
    }
}
