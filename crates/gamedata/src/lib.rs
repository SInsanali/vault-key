//! GameData: reads the user's own installed Borderlands 2 packages (UE3 `.upk`), read-only, and
//! turns the numbers stored in a save's items into what the game shows: display name, rarity
//! level, tier and colour, manufacturer, category, and gear versus not gear.
//!
//! The layers, bottom up:
//! - [`package`]: the LZO wrapper (both layouts), package tables, tagged properties.
//! - [`build_database`]: walks the asset libraries of the base game and of each DLC into an
//!   [`ItemDatabase`].
//! - [`ItemDatabase::info`]: answers for one saved item, given as [`ItemRefs`].
//! - [`ItemDatabase::stats`]: the numbers on the item's card (computed, not yet verified in
//!   game; see `docs/research/2026-11-09-item-stats.md`).
//! - [`GameInstall`], [`DatabaseCache`], [`load_or_import`]: find the install, fingerprint its
//!   packages, and import with a cache kept outside the install.
//!
//! The game install is only ever read. Nothing derived from it is stored anywhere but the
//! cache folder the caller chooses. Parsing never panics on file contents: every read is
//! bounds-checked and failures come back as [`GameDataError`].
//!
//! Format reference: `docs/research/2026-11-08-game-data-spike.md`; what was verified on the
//! Windows build: `docs/research/2026-11-09-windows-game-data.md`.

mod database;
mod error;
mod extract;
mod import;
pub mod package;
#[cfg(test)]
mod test_support;

pub use database::{
    counts_part_slots, rarity_level, AssetCategory, AssetSet, AssetTable, DlcResolution, GearClass,
    ItemCategory, ItemDatabase, ItemInfo, ItemRefs, ObjectInfo, RarityColorRow, RarityInfo,
    RarityTier, RarityValue, Rgba, WeaponKind, PART_SLOTS,
};
pub use database::{
    AttributeEffect, AttributeSource, BaseValueMode, CardAttribute, CardLine, CardLineKind,
    CardText, Comparison, Condition, Conditional, ConditionalCase, CustomPresentation, Formula,
    Initialization, ItemLevel, ItemStats, ModifierKind, Presentation, PresentationEntries,
    PresentationList, PresentationRule, Remap, Rounding, SignStyle, SlotEffect, SlotUpgrade,
    StatDefinition, StatKind, StatRow, StatValue, StatsData, UiStat, ValueData, WeaponBase,
};
pub use error::GameDataError;
pub use extract::build_database;
pub use import::{
    import, load_or_import, DatabaseCache, GameInstall, ImportPhase, ImportProgress, GAME_DATA_ENV,
};
