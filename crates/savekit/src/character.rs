//! Character values and edits (format doc sections 5.1 to 5.12).
//!
//! Numbers are `i64` throughout. A varint from the file is reinterpreted bit for bit, so a
//! nonsense value in a damaged save reads as a nonsense number instead of failing.

use crate::document::SaveDocument;
use crate::error::{Result, SaveKitError};
use crate::field;
use crate::proto::{decode_packed_varints, encode_packed_varints, ProtoField, ProtoMessage, ProtoValue};
use crate::tables::{AMMO_TYPES, MAX_AMMO_SDU_LEVEL, REQUIRED_XP};

/// A currency held in the packed array of field 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Currency {
    Money,
    Eridium,
    Seraph,
    Torgue,
}

impl Currency {
    /// Index into the packed currency array. Index 3 and everything from 5 up belong to
    /// the game and are never changed.
    fn index(self) -> usize {
        match self {
            Currency::Money => 0,
            Currency::Eridium => 1,
            Currency::Seraph => 2,
            Currency::Torgue => 4,
        }
    }
}

pub const MIN_LEVEL: i64 = 1;
pub const MAX_LEVEL: i64 = 80;
pub const MIN_BACKPACK_SLOTS: i64 = 12;
pub const MAX_BACKPACK_SLOTS: i64 = 39;
pub const MIN_BANK_SLOTS: i64 = 6;
pub const MAX_BANK_SLOTS: i64 = 24;
/// Currencies are probably signed 32-bit in the game.
pub const MAX_CURRENCY: i64 = i32::MAX as i64;

/// Field 36 index of the backpack SDU count; the bank's is the next one.
const BACKPACK_SDU: usize = 7;
const BANK_SDU: usize = 8;
pub const AMMO_SDU_COUNT: usize = 7;
const BLACK_MARKET_MAX_LEN: usize = 9;

fn invalid(field: &str, reason: impl Into<String>) -> SaveKitError {
    SaveKitError::invalid(field, reason)
}

/// The nested message in field `number`: empty if the field is absent, an error unless it
/// is exactly one length-delimited entry.
fn existing_sub(m: &ProtoMessage, number: u32) -> Result<ProtoMessage> {
    let mut values = m.values(number);
    match (values.next(), values.next()) {
        (None, _) => Ok(ProtoMessage::default()),
        (Some(ProtoValue::Bytes(bytes)), None) => ProtoMessage::decode(bytes),
        _ => Err(invalid(
            &format!("field {number}"),
            "unexpected encoding (expected a single embedded message); refusing to edit",
        )),
    }
}

/// The packed varints in field `number`, with the same rule as [`existing_sub`].
fn existing_packed(m: &ProtoMessage, number: u32) -> Result<Vec<u64>> {
    let mut values = m.values(number);
    match (values.next(), values.next()) {
        (None, _) => Ok(Vec::new()),
        (Some(ProtoValue::Bytes(bytes)), None) => decode_packed_varints(bytes),
        _ => Err(invalid(
            &format!("field {number}"),
            "unexpected encoding (expected a single packed entry); refusing to edit",
        )),
    }
}

fn edit_sub(m: &mut ProtoMessage, number: u32, body: impl FnOnce(&mut ProtoMessage)) -> Result<()> {
    let mut sub = existing_sub(m, number)?;
    body(&mut sub);
    m.set_single(number, ProtoValue::Bytes(sub.encode()));
    Ok(())
}

fn set_packed(m: &mut ProtoMessage, number: u32, values: &[u64]) {
    m.set_single(number, ProtoValue::Bytes(encode_packed_varints(values)));
}

/// The black-market SDU levels, padded with zeros to `min_len` entries. The game writes only
/// as many entries as it needs (7 in a new character), so edits pad no further than they must.
fn black_market(m: &ProtoMessage, min_len: usize) -> Result<Vec<u64>> {
    let mut levels = existing_packed(m, field::BLACK_MARKET)?;
    if levels.len() > BLACK_MARKET_MAX_LEN {
        return Err(invalid(
            "blackMarket",
            format!("expected at most {BLACK_MARKET_MAX_LEN} entries, found {}", levels.len()),
        ));
    }
    if levels.len() < min_len {
        levels.resize(min_len, 0);
    }
    Ok(levels)
}

/// SDU level (capped at 7) and pool size for ammo type `index`.
fn ammo_values(index: usize, levels: &[u64]) -> (u32, f32) {
    let level = levels.get(index).copied().unwrap_or(0).min(MAX_AMMO_SDU_LEVEL) as u32;
    let ammo = &AMMO_TYPES[index];
    (level, (ammo.base + ammo.step * level) as f32)
}

fn maximize_ammo_in(m: &mut ProtoMessage) -> Result<()> {
    let levels = black_market(m, AMMO_SDU_COUNT)?;
    let mut done = [false; AMMO_SDU_COUNT];
    for entry in m.fields.iter_mut().filter(|f| f.number == field::RESOURCES) {
        let ProtoValue::Bytes(bytes) = &entry.value else { continue };
        let mut resource = ProtoMessage::decode(bytes)?;
        let known = resource.bytes(1).and_then(|name| AMMO_TYPES.iter().position(|a| a.resource.as_bytes() == name));
        let Some(index) = known else { continue };
        let (level, amount) = ammo_values(index, &levels);
        resource.set_single(4, ProtoValue::Varint(u64::from(level)));
        resource.set_single(3, ProtoValue::Fixed32(amount.to_bits()));
        entry.value = ProtoValue::Bytes(resource.encode());
        done[index] = true;
    }
    for (index, ammo) in AMMO_TYPES.iter().enumerate() {
        if done[index] {
            continue;
        }
        let (level, amount) = ammo_values(index, &levels);
        let resource = ProtoMessage::new(vec![
            ProtoField::bytes(1, ammo.resource),
            ProtoField::bytes(2, ammo.pool),
            ProtoField::new(3, ProtoValue::Fixed32(amount.to_bits())),
            ProtoField::varint(4, u64::from(level)),
        ]);
        m.push_repeated(field::RESOURCES, ProtoValue::Bytes(resource.encode()));
    }
    Ok(())
}

impl SaveDocument {
    fn varint(&self, number: u32) -> Option<i64> {
        self.message.varint(number).map(|v| v as i64)
    }

    fn submessage(&self, number: u32) -> Option<ProtoMessage> {
        ProtoMessage::decode(self.message.bytes(number)?).ok()
    }

    // Reads. A missing or unreadable value is 0 (or an empty string).

    /// The character class asset path, such as
    /// `GD_Tulip_Mechromancer.Character.CharClass_Mechromancer`.
    pub fn class_path(&self) -> String {
        String::from_utf8_lossy(self.message.bytes(field::CLASS).unwrap_or_default()).into_owned()
    }

    /// The character name. The game's string encoding is unconfirmed; bytes are read as Latin-1.
    pub fn name(&self) -> String {
        let appearance = self.submessage(field::APPEARANCE);
        let bytes = appearance.as_ref().and_then(|a| a.bytes(1)).unwrap_or_default();
        bytes.iter().map(|&b| char::from(b)).collect()
    }

    pub fn save_game_id(&self) -> i64 {
        self.varint(field::SAVE_GAME_ID).unwrap_or(0)
    }

    pub fn level(&self) -> i64 {
        self.varint(field::LEVEL).unwrap_or(0)
    }

    pub fn experience(&self) -> i64 {
        self.varint(field::EXPERIENCE).unwrap_or(0)
    }

    /// The amount held, read as a signed 32-bit number.
    pub fn currency(&self, currency: Currency) -> i64 {
        let packed = self.message.bytes(field::CURRENCY).and_then(|b| decode_packed_varints(b).ok());
        let value = packed.and_then(|p| p.get(currency.index()).copied()).unwrap_or(0);
        i64::from(value as u32 as i32)
    }

    pub fn money(&self) -> i64 {
        self.currency(Currency::Money)
    }

    pub fn eridium(&self) -> i64 {
        self.currency(Currency::Eridium)
    }

    pub fn seraph(&self) -> i64 {
        self.currency(Currency::Seraph)
    }

    pub fn torgue(&self) -> i64 {
        self.currency(Currency::Torgue)
    }

    pub fn backpack_slots(&self) -> i64 {
        self.submessage(field::SIZES).and_then(|s| s.varint(1)).map_or(0, |v| v as i64)
    }

    pub fn gun_slots(&self) -> i64 {
        self.submessage(field::SIZES).and_then(|s| s.varint(2)).map_or(0, |v| v as i64)
    }

    pub fn bank_slots(&self) -> i64 {
        self.varint(field::BANK_SIZE).unwrap_or(0)
    }

    /// 0 = Normal only, 1 = True Vault Hunter Mode unlocked, 2 = Ultimate unlocked.
    pub fn playthroughs_unlocked(&self) -> i64 {
        self.varint(field::PLAYTHROUGHS_COMPLETED).unwrap_or(0)
    }

    pub fn slaughterdome_unlocked(&self) -> bool {
        self.message.bytes(field::UNLOCKS).is_some_and(|b| b.contains(&1))
    }

    // Name and ID

    /// Sets the character name. It is written as Latin-1, so every character must be U+00FF
    /// or below.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(invalid("name", "must not be empty"));
        }
        let bytes = name
            .chars()
            .map(|c| u8::try_from(u32::from(c)))
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| invalid("name", "contains characters outside Latin-1"))?;
        self.edit(|m| edit_sub(m, field::APPEARANCE, |a| a.set_single(1, ProtoValue::Bytes(bytes))))
    }

    /// Sets the save-game ID, which must match the slot number in the file name.
    pub fn set_save_game_id(&mut self, id: i64) -> Result<()> {
        if id <= 0 {
            return Err(invalid("saveGameID", "must be positive"));
        }
        self.edit(|m| {
            m.set_single(field::SAVE_GAME_ID, ProtoValue::Varint(id as u64));
            Ok(())
        })
    }

    // Level and currency

    /// Sets the level (1 to 80). XP is left alone if it already fits the level, and otherwise
    /// set to the level's minimum. Skill points are not changed.
    pub fn set_level(&mut self, level: i64) -> Result<()> {
        if !(MIN_LEVEL..=MAX_LEVEL).contains(&level) {
            return Err(invalid("level", format!("must be {MIN_LEVEL} to {MAX_LEVEL}")));
        }
        let index = (level - 1) as usize;
        let floor = REQUIRED_XP[index];
        let xp = self.experience();
        // At level 80 there is no next threshold: XP must be exactly the minimum.
        let fits = xp >= floor && REQUIRED_XP.get(index + 1).is_some_and(|&next| xp < next);
        let new_xp = if fits { xp } else { floor };
        self.edit(|m| {
            m.set_single(field::LEVEL, ProtoValue::Varint(level as u64));
            if new_xp != xp || m.first(field::EXPERIENCE).is_none() {
                m.set_single(field::EXPERIENCE, ProtoValue::Varint(new_xp as u64));
            }
            Ok(())
        })
    }

    /// Sets one currency (0 to 2,147,483,647). Every other entry of the array is kept.
    pub fn set_currency(&mut self, currency: Currency, value: i64) -> Result<()> {
        if !(0..=MAX_CURRENCY).contains(&value) {
            return Err(invalid("currency", format!("must be 0 to {MAX_CURRENCY}")));
        }
        self.edit(|m| {
            let mut packed = existing_packed(m, field::CURRENCY)?;
            if packed.len() < 5 {
                packed.resize(5, 0);
            }
            packed[currency.index()] = value as u64;
            set_packed(m, field::CURRENCY, &packed);
            Ok(())
        })
    }

    // Capacity

    /// Sets the backpack size (12 to 39), rounded up to a whole number of SDUs (3 slots each).
    pub fn set_backpack_slots(&mut self, slots: i64) -> Result<()> {
        if !(MIN_BACKPACK_SLOTS..=MAX_BACKPACK_SLOTS).contains(&slots) {
            return Err(invalid("backpackSlots", format!("must be {MIN_BACKPACK_SLOTS} to {MAX_BACKPACK_SLOTS}")));
        }
        let sdu = ((slots - MIN_BACKPACK_SLOTS + 2) / 3) as u64;
        let size = MIN_BACKPACK_SLOTS as u64 + 3 * sdu;
        self.edit(|m| {
            edit_sub(m, field::SIZES, |s| s.set_single(1, ProtoValue::Varint(size)))?;
            let mut levels = black_market(m, BACKPACK_SDU + 1)?;
            levels[BACKPACK_SDU] = sdu;
            set_packed(m, field::BLACK_MARKET, &levels);
            Ok(())
        })
    }

    /// Sets the bank size (6 to 24), rounded up to a whole number of SDUs (2 slots each).
    pub fn set_bank_slots(&mut self, slots: i64) -> Result<()> {
        if !(MIN_BANK_SLOTS..=MAX_BANK_SLOTS).contains(&slots) {
            return Err(invalid("bankSlots", format!("must be {MIN_BANK_SLOTS} to {MAX_BANK_SLOTS}")));
        }
        let sdu = ((slots - MIN_BANK_SLOTS + 1) / 2) as u64;
        let size = MIN_BANK_SLOTS as u64 + 2 * sdu;
        self.edit(|m| {
            m.set_single(field::BANK_SIZE, ProtoValue::Varint(size));
            let mut levels = black_market(m, BANK_SDU + 1)?;
            levels[BANK_SDU] = sdu;
            set_packed(m, field::BLACK_MARKET, &levels);
            Ok(())
        })
    }

    /// Sets the number of weapon slots (2 to 4).
    pub fn set_gun_slots(&mut self, slots: i64) -> Result<()> {
        if !(2..=4).contains(&slots) {
            return Err(invalid("gunSlots", "must be 2 to 4"));
        }
        let extra = (slots - 2) as u64;
        self.edit(|m| {
            edit_sub(m, field::SIZES, |s| {
                s.set_single(2, ProtoValue::Varint(slots as u64));
                // Subfield 3 looks like "extra weapon slots shown"; never leave it above the new count.
                if s.varint(3).is_some_and(|shown| shown as i64 > extra as i64) {
                    s.set_single(3, ProtoValue::Varint(extra));
                }
            })
        })
    }

    // Unlocks

    pub fn unlock_tvhm(&mut self) -> Result<()> {
        self.raise_playthroughs(1)
    }

    pub fn unlock_uvhm(&mut self) -> Result<()> {
        self.raise_playthroughs(2)
    }

    fn raise_playthroughs(&mut self, at_least: i64) -> Result<()> {
        let value = self.playthroughs_unlocked().max(at_least) as u64;
        self.edit(|m| {
            m.set_single(field::PLAYTHROUGHS_COMPLETED, ProtoValue::Varint(value));
            Ok(())
        })
    }

    /// Unlocks the Creature Slaughter Dome (unlock id 1), in the unlock list and the
    /// notification list.
    pub fn unlock_slaughterdome(&mut self) -> Result<()> {
        self.edit(|m| {
            for number in [field::UNLOCKS, field::UNLOCK_NOTIFICATIONS] {
                let mut ids = m.bytes(number).map(<[u8]>::to_vec).unwrap_or_default();
                if !ids.contains(&1) {
                    ids.push(1);
                }
                m.set_single(number, ProtoValue::Bytes(ids));
            }
            Ok(())
        })
    }

    /// Buys every ammo SDU (level 7 for all seven ammo types), then fills the ammo pools.
    pub fn unlock_ammo(&mut self) -> Result<()> {
        self.edit(|m| {
            let mut levels = black_market(m, AMMO_SDU_COUNT)?;
            for level in &mut levels[..AMMO_SDU_COUNT] {
                *level = MAX_AMMO_SDU_LEVEL;
            }
            set_packed(m, field::BLACK_MARKET, &levels);
            maximize_ammo_in(m)
        })
    }

    /// The ammo SDU level (0 to 7) of each ammo type, in [`AMMO_TYPES`] order. A missing or
    /// unreadable table reads as 0.
    pub fn ammo_levels(&self) -> [u64; AMMO_SDU_COUNT] {
        let levels = existing_packed(&self.message, field::BLACK_MARKET).unwrap_or_default();
        std::array::from_fn(|i| levels.get(i).copied().unwrap_or(0).min(MAX_AMMO_SDU_LEVEL))
    }

    /// Sets one ammo type's SDU level (0 to 7; `index` in [`AMMO_TYPES`] order). Its pool,
    /// if the save has one, records the new level and keeps its ammo, trimmed to the new size
    /// if it holds more ([`SaveDocument::maximize_ammo`] fills it). The other types are not
    /// touched.
    pub fn set_ammo_level(&mut self, index: usize, level: u64) -> Result<()> {
        if index >= AMMO_SDU_COUNT {
            return Err(invalid("ammo", format!("must be 0 to {}", AMMO_SDU_COUNT - 1)));
        }
        if level > MAX_AMMO_SDU_LEVEL {
            return Err(invalid("ammoLevel", format!("must be 0 to {MAX_AMMO_SDU_LEVEL}")));
        }
        self.edit(|m| {
            let mut levels = black_market(m, AMMO_SDU_COUNT)?;
            levels[index] = level;
            set_packed(m, field::BLACK_MARKET, &levels);
            let (level, most) = ammo_values(index, &levels);
            let resource_name = AMMO_TYPES[index].resource.as_bytes();
            for entry in m.fields.iter_mut().filter(|f| f.number == field::RESOURCES) {
                let ProtoValue::Bytes(bytes) = &entry.value else { continue };
                let mut resource = ProtoMessage::decode(bytes)?;
                if resource.bytes(1) != Some(resource_name) {
                    continue;
                }
                resource.set_single(4, ProtoValue::Varint(u64::from(level)));
                if let Some(ProtoValue::Fixed32(bits)) = resource.first(3) {
                    if f32::from_bits(*bits) > most {
                        resource.set_single(3, ProtoValue::Fixed32(most.to_bits()));
                    }
                }
                entry.value = ProtoValue::Bytes(resource.encode());
            }
            Ok(())
        })
    }

    /// Fills every ammo pool to the size its SDU level allows, adding pools that are missing.
    /// Field 36 itself is not changed (and not padded).
    pub fn maximize_ammo(&mut self) -> Result<()> {
        self.edit(maximize_ammo_in)
    }
}
