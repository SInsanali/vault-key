//! How an item's rarity level is combined from its definitions (spike doc §5).
//!
//! The level is the sum of every contribution: the item type's `BaseRarity` plus the `Rarity` of
//! each non-empty part slot. The game's `RarityLevelColors` table has rows (6 = e-tech, 7…10 =
//! legendary) that no single definition reaches, so only summation can produce them.

use super::{ItemCategory, RarityValue};

/// The rarity level for a list of contributions, or `None` when it cannot be known.
///
/// Any [`RarityValue::Unresolvable`] contribution, or a sum that is not an exact integer, gives
/// `None`. [`RarityValue::None`] contributes 0, so no contributions at all give `Some(0)`.
pub fn rarity_level(contributions: &[RarityValue]) -> Option<i64> {
    let mut total = 0.0_f64;
    for c in contributions {
        match c {
            RarityValue::None => {}
            RarityValue::Unresolvable => return None,
            RarityValue::Value(v) => total += v,
        }
    }
    // Every integral f64 of this magnitude converts exactly; NaN and infinities fail `fract`.
    (total.fract() == 0.0 && total.abs() < 9.0e15).then_some(total as i64)
}

/// Whether an item's part slots contribute to its rarity (and hold real parts at all).
///
/// Weapons and the gear item classes (shields, grenade mods, class mods, relics) count every
/// non-empty part slot. Usables, customizations and mission items count only their type's base
/// rarity: customization items carry garbage values in their part slots. An unknown category
/// gives `None`, because whether its parts count cannot be decided.
pub fn counts_part_slots(category: ItemCategory) -> Option<bool> {
    match category {
        ItemCategory::Weapon(_)
        | ItemCategory::Shield
        | ItemCategory::GrenadeMod
        | ItemCategory::ClassMod
        | ItemCategory::Relic => Some(true),
        ItemCategory::Usable | ItemCategory::Customization | ItemCategory::Mission => Some(false),
        ItemCategory::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::WeaponKind;
    use RarityValue::{None as Absent, Unresolvable, Value};

    #[test]
    fn sums_contributions() {
        assert_eq!(rarity_level(&[]), Some(0));
        assert_eq!(rarity_level(&[Absent, Absent]), Some(0));
        assert_eq!(rarity_level(&[Value(3.0)]), Some(3));
        assert_eq!(
            rarity_level(&[Absent, Value(4.0), Absent, Value(5.0)]),
            Some(9)
        ); // legendary
        assert_eq!(rarity_level(&[Value(4.0), Value(2.0)]), Some(6)); // e-tech
        assert_eq!(rarity_level(&[Value(502.0), Absent]), Some(502));
    }

    #[test]
    fn unknown_when_unresolvable_or_not_integral() {
        assert_eq!(rarity_level(&[Value(4.0), Unresolvable]), None);
        assert_eq!(rarity_level(&[Unresolvable, Absent]), None);
        assert_eq!(rarity_level(&[Value(2.5)]), None);
        assert_eq!(rarity_level(&[Value(1.25), Value(1.25)]), None);
        assert_eq!(rarity_level(&[Value(f64::NAN)]), None);
        assert_eq!(rarity_level(&[Value(f64::INFINITY)]), None);
        assert_eq!(rarity_level(&[Value(1e300)]), None); // beyond i64: no overflow
        assert_eq!(rarity_level(&[Value(f64::MAX), Value(f64::MAX)]), None);
        assert_eq!(rarity_level(&[Value(-1.0)]), Some(-1));
    }

    /// Ledger ruling (a): which categories' part slots count.
    #[test]
    fn part_slots_count_per_category() {
        use ItemCategory::*;
        for c in [
            Weapon(WeaponKind::Pistol),
            Shield,
            GrenadeMod,
            ClassMod,
            Relic,
        ] {
            assert_eq!(counts_part_slots(c), Some(true), "{c:?}");
        }
        for c in [Usable, Customization, Mission] {
            assert_eq!(counts_part_slots(c), Some(false), "{c:?}");
        }
        assert_eq!(counts_part_slots(Unknown), None);
    }
}
