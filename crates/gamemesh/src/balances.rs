//! Which parts each weapon balance allows in each slot, read from the balance's part-list
//! collections. Used to sweep the part combinations the game can produce; not needed to draw
//! a saved weapon.

use gamedata::package::{PropertyList, UPackage};

use crate::error::Result;

/// The part-list property of each slot, in save order (see [`crate::WEAPON_SLOTS`]).
const SLOT_PROPERTIES: [&str; 9] = [
    "BodyPartData",
    "GripPartData",
    "BarrelPartData",
    "SightPartData",
    "StockPartData",
    "ElementalPartData",
    "Accessory1PartData",
    "Accessory2PartData",
    "MaterialPartData",
];
/// Longest `BaseDefinition` chain followed.
const MAX_CHAIN: usize = 16;

/// One weapon balance with the parts it allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceParts {
    /// The balance definition's path.
    pub balance: String,
    /// The weapon type the balance (or the nearest base that says) builds.
    pub weapon_type: Option<String>,
    /// Part definition paths per slot. A slot takes its list from the nearest definition in
    /// the base chain that enables it.
    pub slots: [Vec<String>; 9],
}

fn slot_list(pkg: &UPackage, collection: &PropertyList, property: &str) -> Result<Option<Vec<String>>> {
    let Some(data) = collection.struct_fields(property) else {
        return Ok(None);
    };
    if data.bool("bEnabled") != Some(true) {
        return Ok(None);
    }
    let mut out = Vec::new();
    if let Some(parts) = data.get("WeightedParts") {
        for entry in pkg.struct_array(parts)? {
            if let Some(path) = entry.object("Part").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty()) {
                out.push(path);
            }
        }
    }
    Ok(Some(out))
}

/// Every `WeaponBalanceDefinition` in `pkg` with its effective part lists. A balance that
/// cannot be read is left out.
pub fn weapon_balances(pkg: &UPackage) -> Vec<BalanceParts> {
    let mut out = Vec::new();
    for &index in pkg.exports_of_class("WeaponBalanceDefinition") {
        let read = || -> Result<BalanceParts> {
            let balance = pkg.export_path(index)?;
            let mut slots: [Option<Vec<String>>; 9] = Default::default();
            let mut weapon_type = None;
            let mut at = Some(index);
            for _ in 0..MAX_CHAIN {
                let Some(current) = at else { break };
                let props = pkg.properties_of_export(current)?;
                if weapon_type.is_none() {
                    weapon_type = props.object("InventoryDefinition").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty());
                }
                if let Some(collection) = props.object("WeaponPartListCollection").filter(|&r| r > 0) {
                    let collection = pkg.properties_of_export((collection - 1) as usize)?;
                    for (slot, property) in SLOT_PROPERTIES.iter().enumerate() {
                        if slots[slot].is_none() {
                            slots[slot] = slot_list(pkg, &collection, property)?;
                        }
                    }
                }
                at = props.object("BaseDefinition").filter(|&r| r > 0).map(|r| (r - 1) as usize).filter(|&next| next != current);
            }
            Ok(BalanceParts { balance, weapon_type, slots: slots.map(Option::unwrap_or_default) })
        };
        if let Ok(parts) = read() {
            out.push(parts);
        }
    }
    out
}
