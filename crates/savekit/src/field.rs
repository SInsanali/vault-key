//! Top-level field numbers of the save's protobuf message (format doc section 4.2).

pub const CLASS: u32 = 1;
pub const LEVEL: u32 = 2;
pub const EXPERIENCE: u32 = 3;
/// Packed varints: money, eridium, seraph crystals, unknown, torgue tokens, and more.
pub const CURRENCY: u32 = 6;
pub const PLAYTHROUGHS_COMPLETED: u32 = 7;
/// Repeated: ammo pools.
pub const RESOURCES: u32 = 11;
/// Message: backpack slots, weapon slots, weapon slots shown.
pub const SIZES: u32 = 13;
/// Message: name and colours.
pub const APPEARANCE: u32 = 19;
pub const SAVE_GAME_ID: u32 = 20;
pub const UNLOCKS: u32 = 23;
pub const UNLOCK_NOTIFICATIONS: u32 = 24;
/// Packed varints: black-market SDU levels.
pub const BLACK_MARKET: u32 = 36;
/// Repeated: items and weapons in the bank.
pub const BANK: u32 = 41;
/// Repeated: items (shields, grenade mods, class mods, relics) and virtual items.
pub const ITEMS: u32 = 53;
/// Repeated: weapons.
pub const WEAPONS: u32 = 54;
pub const BANK_SIZE: u32 = 56;
