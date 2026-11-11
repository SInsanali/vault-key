use crate::items::blob::ItemBlob;

/// Number of part slots in an item or weapon.
pub const PART_COUNT: usize = 11;
const FIELD_COUNT: usize = 6 + PART_COUNT;

/// Value of `set` that marks a virtual item (format doc 5.13).
const VIRTUAL_SET: u32 = 255;

/// Bit widths of the fields, in stream order (format doc 6.6).
fn widths(is_weapon: bool) -> [u32; FIELD_COUNT] {
    let mut widths = [if is_weapon { 17 } else { 16 }; FIELD_COUNT];
    widths[..6].copy_from_slice(&[8, if is_weapon { 13 } else { 17 }, 20, 11, 7, 7]);
    widths
}

/// The fields of an item bitstream (format doc 6.6). The stream is LSB-first and may end
/// early: a field that does not fit is `None`, and so is every field after it.
///
/// Field values are indices into the game's asset libraries; see [`ItemFields::split`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemFields {
    pub set: Option<u32>,
    pub item_type: Option<u32>,
    pub balance: Option<u32>,
    pub manufacturer: Option<u32>,
    /// The item level shown in game.
    pub grade_index: Option<u32>,
    pub game_stage: Option<u32>,
    /// Weapons: body, grip, barrel, sight, stock, elemental, accessory 1, accessory 2,
    /// material, prefix, title. Items: alpha to theta, then material, prefix, title.
    /// An empty slot is all ones.
    pub parts: [Option<u32>; PART_COUNT],
}

impl ItemFields {
    pub fn from_blob(blob: &ItemBlob) -> Self {
        Self::decode(&blob.item_bytes, blob.is_weapon)
    }

    pub fn decode(item_bytes: &[u8], is_weapon: bool) -> Self {
        let total_bits = item_bytes.len().saturating_mul(8);
        let mut values = [None; FIELD_COUNT];
        let mut pos = 0usize;
        for (value, width) in values.iter_mut().zip(widths(is_weapon)) {
            let width = width as usize;
            if pos + width > total_bits {
                break;
            }
            let mut v = 0u32;
            for bit in 0..width {
                let at = pos + bit;
                if (item_bytes[at >> 3] >> (at & 7)) & 1 == 1 {
                    v |= 1 << bit;
                }
            }
            *value = Some(v);
            pos += width;
        }
        let mut parts = [None; PART_COUNT];
        parts.copy_from_slice(&values[6..]);
        ItemFields {
            set: values[0],
            item_type: values[1],
            balance: values[2],
            manufacturer: values[3],
            grade_index: values[4],
            game_stage: values[5],
            parts,
        }
    }

    fn values(&self) -> [Option<u32>; FIELD_COUNT] {
        let mut values = [None; FIELD_COUNT];
        values[..6].copy_from_slice(&[
            self.set,
            self.item_type,
            self.balance,
            self.manufacturer,
            self.grade_index,
            self.game_stage,
        ]);
        values[6..].copy_from_slice(&self.parts);
        values
    }

    /// Packs the fields up to the first `None`, masking each value to its width.
    ///
    /// Pass the item bytes these fields were decoded from as `original` to re-encode an
    /// existing item: if the same number of fields is present as in `original`, the unused
    /// high bits of the last byte and any bytes after it are copied from `original`, so an
    /// unchanged item re-encodes to exactly the same bytes. With `None`, or when fields were
    /// added or removed, the unused bits are zero.
    pub fn encode(&self, is_weapon: bool, original: Option<&[u8]>) -> Vec<u8> {
        let widths = widths(is_weapon);
        let mut out: Vec<u8> = Vec::new();
        let mut bits = 0usize;
        let mut present = 0usize;
        for (value, width) in self.values().into_iter().zip(widths) {
            let Some(value) = value else { break };
            present += 1;
            for bit in 0..width {
                if bits >> 3 == out.len() {
                    out.push(0);
                }
                if (value >> bit) & 1 == 1 {
                    out[bits >> 3] |= 1 << (bits & 7);
                }
                bits += 1;
            }
        }

        if let Some(original) = original {
            let same_shape = Self::decode(original, is_weapon).values().iter().flatten().count() == present;
            if same_shape && original.len() >= out.len() {
                if !bits.is_multiple_of(8) {
                    let last = out.len() - 1;
                    out[last] |= original[last] & (0xFFu8 << (bits % 8));
                }
                out.extend_from_slice(&original[out.len()..]);
            }
        }
        out
    }

    /// A placeholder "virtual" item (the OP level record and its kin): `set` is 255 and
    /// everything else is zero or absent.
    pub fn is_virtual(&self) -> bool {
        let values = self.values();
        values[0] == Some(VIRTUAL_SET) && values[1..].iter().all(|v| v.unwrap_or(0) == 0)
    }

    /// Splits a field value into (library index, asset index); the asset index is the low
    /// `asset_bits` bits.
    pub fn split(value: u32, asset_bits: u32) -> (u32, u32) {
        if asset_bits >= u32::BITS {
            return (0, value);
        }
        (value >> asset_bits, value & ((1 << asset_bits) - 1))
    }
}
