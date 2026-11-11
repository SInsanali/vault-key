//! Walks the asset libraries, the definitions they list and the rarity colour table into an
//! [`ItemDatabase`]. Only the exports the asset tables refer to are parsed (plus the rarity
//! attributes those point to), never every export.

use std::collections::HashMap;

use super::object_index::{Loc, ObjectIndex};
use crate::database::{
    AssetCategory, AssetSet, AssetTable, ItemDatabase, ObjectInfo, RarityColorRow, RarityValue,
    Rgba, WeaponKind,
};
use crate::error::{GameDataError, Result};
use crate::package::{PropertyList, PropertyValue, UPackage};

const GLOBALS_PATH: &str = "GD_Globals.General.Globals";

/// The base game's libraries, in `AssetLibraries[0…5]` order ([`AssetCategory::ALL`] order).
const BASE_LIBRARIES: [&str; 6] = [
    "GD_AssetLibraries.Inventory.AL_WeaponTypes",
    "GD_AssetLibraries.Inventory.AL_WeaponParts",
    "GD_AssetLibraries.Inventory.AL_ItemTypes",
    "GD_AssetLibraries.Inventory.AL_ItemParts",
    "GD_AssetLibraries.Inventory.AL_Manufacturer",
    "GD_AssetLibraries.Inventory.AL_Balance",
];

/// Each library's `LibraryType` class, in the same order. A DLC's set lists its six libraries
/// in this order; the `LibraryType` of each library and sublibrary is checked where present
/// (several DLC libraries have none, but every sublibrary does).
const LIBRARY_TYPES: [&str; 6] = [
    "WeaponTypeDefinition",
    "WeaponPartDefinition",
    "ItemDefinition",
    "ItemPartDefinition",
    "ManufacturerDefinition",
    "InventoryBalanceDefinition",
];

const DLC_SET_CLASS: &str = "DownloadableAssetLibraryDefinition";
/// Elements of the static `PathComponentNames` array in an `AssetPaths` entry.
const PATH_COMPONENTS: usize = 6;

/// Builds the item database from decompressed packages: the base game's `Startup.upk` plus any
/// DLC startup packages. When two packages define the same object path, the earlier one wins.
///
/// The base game's globals and six asset libraries must be present, or this fails. Every
/// `DownloadableAssetLibraryDefinition` found becomes the set numbered by its package
/// definition's `PackageId`; one that cannot be read is recorded in
/// [`ItemDatabase::skipped_packages`] instead of failing the build.
pub fn build_database(packages: &[UPackage], fingerprint: &str) -> Result<ItemDatabase> {
    let ctx = BuildContext::new(packages);
    let globals_loc = ctx
        .index
        .locate(GLOBALS_PATH)
        .ok_or_else(|| GameDataError::UnexpectedData(format!("{GLOBALS_PATH} not found")))?;
    let globals = ctx.properties(globals_loc)?;
    let mut db = ItemDatabase::new(fingerprint);

    let mut base = AssetSet::new(None);
    for category in AssetCategory::ALL {
        let i = category.index();
        let library = globals
            .object_at("AssetLibraries", i)
            .and_then(|r| ctx.resolve(r, globals_loc.package))
            .filter(|&l| ctx.path(l).is_ok_and(|p| p == BASE_LIBRARIES[i]))
            .ok_or_else(|| {
                GameDataError::UnexpectedData(format!(
                    "AssetLibraries[{i}] is not {}",
                    BASE_LIBRARIES[i]
                ))
            })?;
        let (tables, unresolved) = ctx.read_library(library, category, true)?;
        base.set_tables(category, tables);
        base.set_unresolved_sublibraries(category, unresolved);
    }
    ctx.add_objects(&base, &mut db.objects);
    db.sets.insert(0, base);

    for (package, p) in packages.iter().enumerate() {
        for &export in p.exports_of_class(DLC_SET_CLASS) {
            match ctx.read_dlc_set(Loc { package, export }) {
                Ok((id, set)) => match db.sets.get(&id) {
                    None => {
                        ctx.add_objects(&set, &mut db.objects);
                        db.sets.insert(id, set);
                    }
                    // Another package carrying a copy of the same set is harmless.
                    Some(existing) if *existing == set => {}
                    Some(_) => db.skipped_packages.push(format!(
                        "{}: a different set {id} is already defined",
                        p.name
                    )),
                },
                Err(e) => db.skipped_packages.push(format!("{}: {e}", p.name)),
            }
        }
    }

    db.rarity_colors = rarity_colors(&globals, &packages[globals_loc.package])?;
    db.stat_data = super::stats::extract_stats(
        &ctx,
        db.objects.keys().map(String::as_str),
        &globals,
        globals_loc.package,
    );
    Ok(db)
}

/// `RarityLevelColors`: `Color` is a native FColor stored as B,G,R,A.
fn rarity_colors(globals: &PropertyList, pkg: &UPackage) -> Result<Vec<RarityColorRow>> {
    let value = globals.get("RarityLevelColors").ok_or_else(|| {
        GameDataError::UnexpectedData(format!("{GLOBALS_PATH} has no RarityLevelColors"))
    })?;
    pkg.struct_array(value)?
        .iter()
        .map(|row| {
            let color = match row.get("Color") {
                Some(PropertyValue::StructRaw { name, bytes }) if name == "Color" => {
                    <[u8; 4]>::try_from(bytes.as_slice()).ok()
                }
                _ => None,
            };
            let [b, g, r, a] = color.ok_or_else(|| {
                GameDataError::UnexpectedData("RarityLevelColors row without a 4-byte Color".into())
            })?;
            Ok(RarityColorRow {
                min_level: row.int("MinLevel").unwrap_or(0),
                max_level: row.int("MaxLevel").unwrap_or(0),
                color: Rgba::new(r, g, b, a),
                rating: row
                    .enum_name("RarityRating")
                    .unwrap_or("RARITY_Unknown")
                    .to_string(),
            })
        })
        .collect()
}

/// Packages plus a cross-package path index; resolves references and reads definitions.
pub(crate) struct BuildContext<'a> {
    pub(crate) packages: &'a [UPackage],
    pub(crate) index: ObjectIndex<'a>,
}

impl<'a> BuildContext<'a> {
    pub(crate) fn new(packages: &'a [UPackage]) -> Self {
        Self {
            packages,
            index: ObjectIndex::new(packages),
        }
    }

    /// Resolves an object reference made from inside package `package`. Imports are located by
    /// path across all the packages.
    pub(crate) fn resolve(&self, reference: i32, package: usize) -> Option<Loc> {
        let p = self.packages.get(package)?;
        match reference {
            0 => None,
            r if r > 0 => {
                let export = r as usize - 1;
                (export < p.exports.len()).then_some(Loc { package, export })
            }
            r => self.index.locate(&p.path_of(r).ok()?),
        }
    }

    pub(crate) fn path(&self, loc: Loc) -> Result<String> {
        self.packages[loc.package].export_path(loc.export)
    }

    pub(crate) fn class_name(&self, loc: Loc) -> &'a str {
        self.packages[loc.package].class_name_of_export(loc.export)
    }

    pub(crate) fn properties(&self, loc: Loc) -> Result<PropertyList> {
        self.packages[loc.package].properties_of_export(loc.export)
    }

    /// Stores an [`ObjectInfo`] for every table path that is found in the packages and is not
    /// already known. Paths that are not found are simply absent.
    fn add_objects(&self, set: &AssetSet, objects: &mut HashMap<String, ObjectInfo>) {
        for category in AssetCategory::ALL {
            for path in set.paths(category) {
                if objects.contains_key(path) {
                    continue;
                }
                let info = self
                    .index
                    .locate(path)
                    .and_then(|loc| self.info(loc, category));
                if let Some(info) = info {
                    objects.insert(path.to_string(), info);
                }
            }
        }
    }

    /// A DLC's set: its `PackageId` and the tables of its six libraries.
    fn read_dlc_set(&self, definition: Loc) -> Result<(u32, AssetSet)> {
        let what = self.path(definition)?;
        let props = self.properties(definition)?;
        let package_def = props
            .object("PackageDef")
            .and_then(|r| self.resolve(r, definition.package))
            .ok_or_else(|| GameDataError::UnexpectedData(format!("{what}: no PackageDef")))?;
        let def_props = self.properties(package_def)?;
        // 0 is the base game and 255 marks virtual items; the saved set id is 8 bits.
        let id = def_props
            .int("PackageId")
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| (1..255).contains(id))
            .ok_or_else(|| GameDataError::UnexpectedData(format!("{what}: bad PackageId")))?;

        let mut set = AssetSet::new(def_props.string("DLCName").map(str::to_string));
        for category in AssetCategory::ALL {
            let i = category.index();
            let library = props
                .object_at("Libraries", i)
                .and_then(|r| self.resolve(r, definition.package))
                .ok_or_else(|| {
                    GameDataError::UnexpectedData(format!("{what}: no Libraries[{i}]"))
                })?;
            let (tables, unresolved) = self.read_library(library, category, false)?;
            set.set_tables(category, tables);
            set.set_unresolved_sublibraries(category, unresolved);
        }
        Ok((id, set))
    }

    /// One table per `SublibraryLinks` entry. A null (or unlocatable) link yields an empty table
    /// and records the sublibrary's name from `Sublibraries`. A DLC library with no sublibraries
    /// has no `SublibraryLinks` property at all; the base game's must have one.
    fn read_library(
        &self,
        library: Loc,
        category: AssetCategory,
        require_links: bool,
    ) -> Result<(Vec<AssetTable>, Vec<String>)> {
        let pkg = &self.packages[library.package];
        let props = self.properties(library)?;
        self.check_library_type(library, &props, category)?;
        let links = match props.get("SublibraryLinks") {
            Some(value) => pkg.object_array(value)?,
            None if require_links => {
                return Err(GameDataError::UnexpectedData(format!(
                    "{} has no SublibraryLinks",
                    self.path(library)?
                )));
            }
            None => Vec::new(),
        };
        let names = match props.get("Sublibraries") {
            Some(value) => pkg.string_array(value)?,
            None => Vec::new(),
        };
        if names.len() != links.len() {
            return Err(GameDataError::UnexpectedData(format!(
                "{}: {} Sublibraries for {} links",
                self.path(library)?,
                names.len(),
                links.len()
            )));
        }
        let mut tables = Vec::with_capacity(links.len());
        let mut unresolved = Vec::new();
        for (link, name) in links.into_iter().zip(names) {
            match self.resolve(link, library.package) {
                Some(sublibrary) => tables.push(self.read_sublibrary(sublibrary, category)?),
                None => {
                    unresolved.push(name);
                    tables.push(Vec::new());
                }
            }
        }
        Ok((tables, unresolved))
    }

    /// A library or sublibrary whose `LibraryType` names a class must name its category's
    /// class, so a set that lists its libraries in another order is rejected, not misread.
    fn check_library_type(
        &self,
        loc: Loc,
        props: &PropertyList,
        category: AssetCategory,
    ) -> Result<()> {
        let Some(reference) = props.object("LibraryType").filter(|&r| r != 0) else {
            return Ok(());
        };
        let found = self.packages[loc.package].path_of(reference)?;
        let expected = LIBRARY_TYPES[category.index()];
        if found.rsplit('.').next() == Some(expected) {
            Ok(())
        } else {
            Err(GameDataError::UnexpectedData(format!(
                "{} holds {found}, not {expected}",
                self.path(loc)?
            )))
        }
    }

    /// Each `AssetPaths` entry: the sublibrary's first path component plus the non-`None`
    /// `PathComponentNames`, joined with "."; `None` when every component is `None`.
    fn read_sublibrary(&self, sublibrary: Loc, category: AssetCategory) -> Result<AssetTable> {
        let path = self.path(sublibrary)?;
        let root = path.split('.').next().unwrap_or(&path);
        let props = self.properties(sublibrary)?;
        self.check_library_type(sublibrary, &props, category)?;
        let Some(value) = props.get("AssetPaths") else {
            return Ok(Vec::new());
        };
        self.packages[sublibrary.package]
            .struct_array(value)?
            .iter()
            .map(|entry| {
                // A set bit would mean a component is a subobject, joined some other way. The
                // game's own data has none, so stop instead of guessing.
                let mask = match entry.get("IsSubobjectMask") {
                    None => 0,
                    Some(PropertyValue::Byte(b)) => i64::from(*b),
                    Some(PropertyValue::Int(v)) => i64::from(*v),
                    Some(other) => {
                        return Err(GameDataError::UnexpectedData(format!(
                            "{path}: IsSubobjectMask {other:?}"
                        )));
                    }
                };
                if mask != 0 {
                    return Err(GameDataError::UnexpectedData(format!(
                        "{path}: nonzero IsSubobjectMask {mask}"
                    )));
                }
                let mut parts = vec![root];
                parts.extend(
                    (0..PATH_COMPONENTS)
                        .filter_map(|i| entry.name_at("PathComponentNames", i))
                        .filter(|&n| n != "None"),
                );
                Ok((parts.len() > 1).then(|| parts.join(".")))
            })
            .collect()
    }

    /// The [`ObjectInfo`] for a table entry. `None` when the definition's class does not belong
    /// in the table's category, or the definition cannot be read.
    ///
    /// The base game's tables hold only classes that fit. A few DLC tables list an object in the
    /// wrong table (a balance among the parts, a shield type among the balances); such an entry
    /// is left out, so a reference to it resolves to nothing instead of to the wrong kind.
    fn info(&self, loc: Loc, category: AssetCategory) -> Option<ObjectInfo> {
        let class_name = self.class_name(loc);
        if kind_of_class(class_name) != kind_of_category(category) {
            return None;
        }
        let p = self.properties(loc).ok()?;
        let text = |name: &str| p.string(name).map(str::to_string);
        Some(match category {
            AssetCategory::WeaponTypes => ObjectInfo::WeaponType {
                kind: weapon_kind(p.get("WeaponType")),
                type_name: text("Typename"),
                base_rarity: self.rarity(p.get("BaseRarity"), loc.package),
            },
            AssetCategory::ItemTypes => ObjectInfo::ItemType {
                class_name: class_name.to_string(),
                item_name: text("ItemName"),
                name_is_full: p.bool("bItemNameIsFullName").unwrap_or(false),
                base_rarity: self.rarity(p.get("BaseRarity"), loc.package),
            },
            AssetCategory::WeaponParts | AssetCategory::ItemParts => ObjectInfo::Part {
                class_name: class_name.to_string(),
                part_name: text("PartName"),
                rarity: self.rarity(p.get("Rarity"), loc.package),
            },
            AssetCategory::Manufacturers => {
                let grades = p
                    .get("Grades")
                    .and_then(|v| self.packages[loc.package].struct_array(v).ok());
                ObjectInfo::Manufacturer {
                    display_name: grades
                        .as_ref()
                        .and_then(|g| g.first())
                        .and_then(|g| g.string("DisplayName"))
                        .map(str::to_string),
                }
            }
            AssetCategory::Balances => ObjectInfo::Balance {
                class_name: class_name.to_string(),
            },
        })
    }

    /// Resolves an `AttributeInitializationData` struct statically.
    ///
    /// The value is the `ConstantValue` of `BaseValueAttribute`'s single constant resolver when
    /// an attribute is set, and `BaseValueConstant` otherwise: the attribute replaces the
    /// constant, it is not added to it. (The base game never sets both. The five DLC
    /// definitions that do are all effervescent items whose attribute is the effervescent
    /// level, 506; adding their constants would give levels the colour table does not have.)
    ///
    /// Anything else the struct could do (an initialization definition, a scale other than 1,
    /// any other resolver chain) makes the value unresolvable.
    pub(crate) fn rarity(&self, value: Option<&PropertyValue>, package: usize) -> RarityValue {
        let s = match value {
            None => return RarityValue::None,
            Some(PropertyValue::Struct(s)) => s,
            Some(_) => return RarityValue::Unresolvable,
        };
        if !matches!(
            s.get("InitializationDefinition"),
            None | Some(PropertyValue::Object(0))
        ) {
            return RarityValue::Unresolvable;
        }
        match s.get("BaseValueScaleConstant") {
            None => {}
            Some(PropertyValue::Float(scale)) if *scale == 1.0 => {}
            Some(_) => return RarityValue::Unresolvable,
        }
        let constant = match s.get("BaseValueConstant") {
            None => 0.0,
            Some(PropertyValue::Float(f)) => f64::from(*f),
            Some(_) => return RarityValue::Unresolvable,
        };
        let total = match s.get("BaseValueAttribute") {
            None | Some(PropertyValue::Object(0)) => constant,
            Some(PropertyValue::Object(reference)) => {
                match self.constant_value_of_attribute(*reference, package) {
                    Some(value) => value,
                    None => return RarityValue::Unresolvable,
                }
            }
            Some(_) => return RarityValue::Unresolvable,
        };
        if total.is_finite() {
            RarityValue::Value(total)
        } else {
            RarityValue::Unresolvable
        }
    }

    /// The attribute's `ValueResolverChain` must be exactly one `ConstantAttributeValueResolver`
    /// with an explicit `ConstantValue`.
    fn constant_value_of_attribute(&self, reference: i32, package: usize) -> Option<f64> {
        let attribute = self.resolve(reference, package)?;
        let props = self.properties(attribute).ok()?;
        let chain = self.packages[attribute.package]
            .object_array(props.get("ValueResolverChain")?)
            .ok()?;
        let [resolver] = chain[..] else {
            return None;
        };
        let resolver = self.resolve(resolver, attribute.package)?;
        if self.class_name(resolver) != "ConstantAttributeValueResolver" {
            return None;
        }
        self.properties(resolver)
            .ok()?
            .float("ConstantValue")
            .map(f64::from)
    }
}

/// The kinds of definition the asset tables hold. Which kind a class is does not depend on the
/// table that lists it, so an object listed by two tables is described the same way by both.
#[derive(PartialEq, Eq)]
enum DefinitionKind {
    WeaponType,
    ItemType,
    Part,
    Manufacturer,
    Balance,
}

fn kind_of_category(category: AssetCategory) -> DefinitionKind {
    match category {
        AssetCategory::WeaponTypes => DefinitionKind::WeaponType,
        AssetCategory::ItemTypes => DefinitionKind::ItemType,
        AssetCategory::WeaponParts | AssetCategory::ItemParts => DefinitionKind::Part,
        AssetCategory::Manufacturers => DefinitionKind::Manufacturer,
        AssetCategory::Balances => DefinitionKind::Balance,
    }
}

/// Parts and balances are recognised by their class-name suffix (`WeaponPartDefinition`,
/// `ItemNamePartDefinition`, `ClassModBalanceDefinition`, …). Any other class counts as an item
/// type; one that is not really an item gets [`crate::ItemCategory::Unknown`] later.
fn kind_of_class(class_name: &str) -> DefinitionKind {
    match class_name {
        "WeaponTypeDefinition" => DefinitionKind::WeaponType,
        "ManufacturerDefinition" => DefinitionKind::Manufacturer,
        c if c.ends_with("PartDefinition") => DefinitionKind::Part,
        c if c.ends_with("BalanceDefinition") => DefinitionKind::Balance,
        _ => DefinitionKind::ItemType,
    }
}

/// `WeaponType` is omitted for pistols (the enum's default value).
fn weapon_kind(value: Option<&PropertyValue>) -> Option<WeaponKind> {
    let name = match value {
        None => return Some(WeaponKind::Pistol),
        Some(PropertyValue::EnumName(name)) => name.as_str(),
        Some(_) => return None,
    };
    match name {
        "WT_Shotgun" => Some(WeaponKind::Shotgun),
        "WT_SMG" => Some(WeaponKind::Smg),
        "WT_AssaultRifle" => Some(WeaponKind::AssaultRifle),
        "WT_SniperRifle" => Some(WeaponKind::SniperRifle),
        "WT_RocketLauncher" => Some(WeaponKind::Launcher),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::DlcResolution;
    use crate::test_support::{
        asset_path, base_world, dlc_set, dlc_world, p, rarity_constant, Pkg, World, LIBRARY_TYPES,
        P, V,
    };

    fn parse(name: &str, world: &World) -> UPackage {
        UPackage::parse(name, world.build()).unwrap()
    }

    fn build(packages: &[UPackage]) -> ItemDatabase {
        build_database(packages, "test").unwrap()
    }

    fn paths(set: &AssetSet, category: AssetCategory) -> Vec<Vec<Option<&str>>> {
        set.tables(category)
            .iter()
            .map(|t| t.iter().map(|p| p.as_deref()).collect())
            .collect()
    }

    #[test]
    fn missing_globals_is_an_error() {
        let mut b = Pkg::new();
        b.raw_export(0, 0, "Unrelated", vec![0; 12]);
        let pkg = UPackage::parse("t", b.build()).unwrap();
        assert!(matches!(
            build_database(&[pkg], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));
        assert!(matches!(
            build_database(&[], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));
    }

    #[test]
    fn object_index_first_package_wins() {
        let package = |name: &str, objects: &[&str]| {
            let mut b = Pkg::new();
            for object in objects {
                b.raw_export(0, 0, object, vec![]);
            }
            UPackage::parse(name, b.build()).unwrap()
        };
        let packages = [
            package("a", &["Shared", "OnlyA"]),
            package("b", &["Other", "Shared"]),
        ];
        let index = ObjectIndex::new(&packages);
        assert_eq!(
            index.locate("Shared"),
            Some(Loc {
                package: 0,
                export: 0
            })
        );
        assert_eq!(
            index.locate("Other"),
            Some(Loc {
                package: 1,
                export: 0
            })
        );
        assert_eq!(index.locate("Missing"), None);
    }

    #[test]
    fn builds_the_base_game() {
        let db = build(&[parse("Startup.upk", &base_world())]);
        assert_eq!(db.schema, ItemDatabase::SCHEMA_VERSION);
        assert_eq!(db.fingerprint, "test");
        assert!(db.skipped_packages.is_empty());
        assert_eq!(db.sets.keys().copied().collect::<Vec<_>>(), [0]);
        let base = &db.sets[&0];
        assert_eq!(base.dlc_name, None);

        use AssetCategory::*;
        assert_eq!(
            paths(base, WeaponTypes),
            [[Some("GD_Weap.A.Pistol"), Some("GD_Weap.A.Sniper")]]
        );
        // The entry whose components are all `None` has no path.
        assert_eq!(
            paths(base, WeaponParts)[0][6..],
            [Some("GD_Weap.Parts.Grip"), None]
        );
        // A null link gives an empty table and records the sublibrary's name.
        assert_eq!(paths(base, ItemTypes)[1], []);
        assert_eq!(
            base.unresolved_sublibraries(ItemTypes),
            ["GD_Gone.PackageAssetSublibrary_9"]
        );
        assert!(base.unresolved_sublibraries(Balances).is_empty());
        assert_eq!(paths(base, Balances)[1], [Some("GD_Items.Bal.Shield")]);

        // A table path with no object in the packages is simply absent.
        assert_eq!(db.path(ItemParts, 2, 0), Some("GD_Items.Parts.NotThere"));
        assert_eq!(db.object("GD_Items.Parts.NotThere"), None);

        assert_eq!(
            db.object("GD_Weap.A.Pistol"),
            Some(&ObjectInfo::WeaponType {
                kind: Some(WeaponKind::Pistol),
                type_name: Some("Pistol".into()),
                base_rarity: RarityValue::None
            })
        );
        assert_eq!(
            db.object("GD_Weap.A.Sniper"),
            Some(&ObjectInfo::WeaponType {
                kind: Some(WeaponKind::SniperRifle),
                type_name: Some("Sniper Rifle".into()),
                base_rarity: RarityValue::None
            })
        );
        assert_eq!(
            db.object("GD_Weap.Parts.Barrel5"),
            Some(&ObjectInfo::Part {
                class_name: "WeaponPartDefinition".into(),
                part_name: None,
                rarity: RarityValue::Value(5.0)
            })
        );
        assert_eq!(
            db.object("GD_Weap.Parts.Title_Shot"),
            Some(&ObjectInfo::Part {
                class_name: "WeaponNamePartDefinition".into(),
                part_name: Some("Shot".into()),
                rarity: RarityValue::None
            })
        );
        assert_eq!(
            db.object("GD_Items.Types.Echo"),
            Some(&ObjectInfo::ItemType {
                class_name: "UsableItemDefinition".into(),
                item_name: Some("Helmet Data".into()),
                name_is_full: true,
                base_rarity: RarityValue::Value(502.0)
            })
        );
        // The manufacturer's name is its first grade's display name.
        assert_eq!(
            db.object("GD_Manufacturers.M.Maker"),
            Some(&ObjectInfo::Manufacturer {
                display_name: Some("Maker".into())
            })
        );
        assert_eq!(
            db.object("GD_Weap.Bal.Mission"),
            Some(&ObjectInfo::Balance {
                class_name: "MissionWeaponBalanceDefinition".into()
            })
        );
        assert_eq!(db.objects.len(), 18);

        // Colours are stored B,G,R,A; missing levels default to 0.
        assert_eq!(db.rarity_colors.len(), 9);
        assert_eq!(
            db.rarity_colors[0],
            RarityColorRow {
                min_level: 0,
                max_level: 0,
                color: Rgba::new(205, 193, 175, 0),
                rating: "RARITY_Unknown".into()
            }
        );
        assert_eq!(db.color_row(2).unwrap().color.hex(), "#3DD20B");
        let nine = db.color_row(9).unwrap();
        assert_eq!(
            (nine.min_level, nine.max_level, nine.rating.as_str()),
            (7, 10, "RARITY_Legendary")
        );
        assert_eq!(db.color_row(11), None);
    }

    #[test]
    fn base_libraries_must_be_the_expected_ones() {
        // The globals name a library at another path.
        let mut w = base_world();
        let stray = w.library("GD_Other.AL_Balance", None, &[]);
        let mut props: Vec<P> = (0..5)
            .map(|i| {
                let name = [
                    "WeaponTypes",
                    "WeaponParts",
                    "ItemTypes",
                    "ItemParts",
                    "Manufacturer",
                ][i];
                P(
                    "AssetLibraries",
                    i as i32,
                    V::Obj(w.get(&format!("GD_AssetLibraries.Inventory.AL_{name}"))),
                )
            })
            .collect();
        props.push(P("AssetLibraries", 5, V::Obj(stray)));
        props.push(p("RarityLevelColors", V::Structs(vec![])));
        let globals = w.get("GD_Globals.General.Globals");
        w.pkg.set_export_props(globals, &props);
        let error = build_database(&[parse("s", &w)], "x").unwrap_err();
        assert_eq!(
            error,
            GameDataError::UnexpectedData(
                "AssetLibraries[5] is not GD_AssetLibraries.Inventory.AL_Balance".into()
            )
        );

        // A base library without links, and one whose names and links differ in number.
        let mut w = base_world();
        let library = w.get("GD_AssetLibraries.Inventory.AL_Manufacturer");
        w.pkg.set_export_props(library, &[]);
        assert!(matches!(
            build_database(&[parse("s", &w)], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));
        let mut w = base_world();
        let library = w.get("GD_AssetLibraries.Inventory.AL_Manufacturer");
        let sub = w.get("GD_Manufacturers.Sub");
        w.pkg.set_export_props(
            library,
            &[
                p("Sublibraries", V::Strs(vec!["a", "b"])),
                p("SublibraryLinks", V::Objs(vec![sub])),
            ],
        );
        assert!(matches!(
            build_database(&[parse("s", &w)], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));

        // No colour table.
        let mut w = base_world();
        let globals = w.get("GD_Globals.General.Globals");
        let libraries: Vec<P> = [
            "WeaponTypes",
            "WeaponParts",
            "ItemTypes",
            "ItemParts",
            "Manufacturer",
            "Balance",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| {
            P(
                "AssetLibraries",
                i as i32,
                V::Obj(w.get(&format!("GD_AssetLibraries.Inventory.AL_{name}"))),
            )
        })
        .collect();
        w.pkg.set_export_props(globals, &libraries);
        assert!(matches!(
            build_database(&[parse("s", &w)], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));
    }

    #[test]
    fn subobject_masks_and_wrong_library_types_stop_the_base_build() {
        let mut w = base_world();
        let sub = w.get("GD_Manufacturers.Sub");
        let manufacturer = w.class(LIBRARY_TYPES[4]);
        w.pkg.set_export_props(
            sub,
            &[
                p("LibraryType", V::Obj(manufacturer)),
                p(
                    "AssetPaths",
                    V::Structs(vec![asset_path(&["M", "Maker"], 1)]),
                ),
            ],
        );
        let error = build_database(&[parse("s", &w)], "x").unwrap_err();
        assert_eq!(
            error,
            GameDataError::UnexpectedData("GD_Manufacturers.Sub: nonzero IsSubobjectMask 1".into())
        );

        // A sublibrary of another category's type linked into a library.
        let mut w = base_world();
        let sub = w.get("GD_Manufacturers.Sub");
        let wrong = w.class(LIBRARY_TYPES[5]);
        w.pkg.set_export_props(
            sub,
            &[
                p("LibraryType", V::Obj(wrong)),
                p(
                    "AssetPaths",
                    V::Structs(vec![asset_path(&["M", "Maker"], 0)]),
                ),
            ],
        );
        assert!(matches!(
            build_database(&[parse("s", &w)], "x"),
            Err(GameDataError::UnexpectedData(_))
        ));
    }

    #[test]
    fn builds_dlc_sets() {
        let packages = [
            parse("Startup.upk", &base_world()),
            parse("Tulip_Startup_SF.upk", &dlc_world(2)),
        ];
        let db = build(&packages);
        assert!(db.skipped_packages.is_empty(), "{:?}", db.skipped_packages);
        assert_eq!(db.sets.keys().copied().collect::<Vec<_>>(), [0, 2]);
        let set = &db.sets[&2];
        assert_eq!(set.dlc_name.as_deref(), Some("Tulip"));

        use AssetCategory::*;
        // Libraries with no `SublibraryLinks` are empty, not errors.
        assert!(set.tables(WeaponTypes).is_empty());
        assert!(set.tables(Manufacturers).is_empty());
        assert_eq!(set.tables(WeaponParts).len(), 1);
        assert_eq!(set.tables(WeaponParts)[0].len(), 4);

        // Values of a DLC set carry the top lib bit; base values do not.
        let flag = 1 << (WeaponParts.lib_bits() - 1 + WeaponParts.asset_bits());
        assert_eq!(
            db.value_of("GD_Tulip.Parts.Body500", WeaponParts, 2),
            Some(flag | 1)
        );
        assert_eq!(db.value_of("GD_Tulip.Parts.Body500", WeaponParts, 0), None);
        assert_eq!(db.value_of("GD_Weap.Parts.Body4", WeaponParts, 0), Some(1));
        assert_eq!(db.value_of("GD_Weap.Parts.Body4", WeaponParts, 2), None);
        assert_eq!(db.value_of("GD_Weap.Parts.Body4", WeaponParts, 9), None);

        let verified = DlcResolution::Verified;
        assert_eq!(
            db.path_with(WeaponParts, flag | 1, 2, verified),
            Some("GD_Tulip.Parts.Body500")
        );
        // Without the flag, an item of set 2 refers to the base game.
        assert_eq!(
            db.path_with(WeaponParts, 1, 2, verified),
            Some("GD_Weap.Parts.Body4")
        );
        // An item of set 0 never reaches a DLC set, and an unknown set has no tables.
        assert_eq!(db.path_with(WeaponParts, flag | 1, 0, verified), None);
        assert_eq!(db.path_with(WeaponParts, flag | 1, 3, verified), None);
        assert_eq!(
            db.path_with(WeaponParts, 1, 3, verified),
            Some("GD_Weap.Parts.Body4")
        );
        // The policy: nothing of a DLC set resolves until verified.
        assert_eq!(ItemDatabase::DLC_RESOLUTION, DlcResolution::Unverified);
        assert_eq!(db.path(WeaponParts, flag | 1, 2), None);
        assert_eq!(db.path(WeaponParts, 1, 2), None);
        assert_eq!(db.path(WeaponParts, 1, 0), Some("GD_Weap.Parts.Body4"));

        // Rarity through an attribute defined in the DLC package.
        assert_eq!(
            db.object("GD_Tulip.Parts.BarrelSeraph"),
            Some(&ObjectInfo::Part {
                class_name: "WeaponPartDefinition".into(),
                part_name: None,
                rarity: RarityValue::Value(497.0)
            })
        );
        // A balance filed among the parts is a balance, never a part.
        assert_eq!(
            set.tables(WeaponParts)[0][3].as_deref(),
            Some("GD_Tulip.Bal.Gun")
        );
        assert_eq!(
            db.object("GD_Tulip.Bal.Gun"),
            Some(&ObjectInfo::Balance {
                class_name: "WeaponBalanceDefinition".into()
            })
        );
        assert!(matches!(
            db.object("GD_Tulip.Types.ClassMod"),
            Some(ObjectInfo::ItemType { class_name, .. }) if class_name == "CrossDLCClassModDefinition"
        ));
        assert_eq!(db.objects.len(), 18 + 6);
    }

    #[test]
    fn mis_filed_objects_are_left_out() {
        // The only table that lists the balance is a parts table.
        let mut w = World::new();
        w.object("GD_X.Bal.Gun", "WeaponBalanceDefinition", &[]);
        w.object("GD_X.Types.Shield", "ShieldDefinition", &[]);
        w.object("GD_X.Parts.Body", "WeaponPartDefinition", &[]);
        let parts = w.sublibrary(
            "GD_X.Sub_Parts",
            None,
            &[&["Bal", "Gun"], &["Types", "Shield"], &["Parts", "Body"]],
        );
        let balances = w.sublibrary(
            "GD_X.Sub_Bal",
            None,
            &[&["Types", "Shield"], &["Parts", "Body"]],
        );
        let libraries = [
            w.library("GD_X.AL_WT", None, &[]),
            w.library("GD_X.AL_WP", None, &[("GD_X.Sub_Parts", parts)]),
            w.library("GD_X.AL_IT", None, &[]),
            w.library("GD_X.AL_IP", None, &[]),
            w.library("GD_X.AL_M", None, &[]),
            w.library("GD_X.AL_B", None, &[("GD_X.Sub_Bal", balances)]),
        ];
        dlc_set(&mut w, "X", Some(7), libraries);
        let db = build(&[parse("Startup.upk", &base_world()), parse("X.upk", &w)]);
        assert!(db.skipped_packages.is_empty(), "{:?}", db.skipped_packages);
        assert_eq!(db.sets[&7].tables(AssetCategory::WeaponParts)[0].len(), 3);
        assert_eq!(db.object("GD_X.Bal.Gun"), None);
        assert_eq!(db.object("GD_X.Types.Shield"), None);
        assert!(matches!(
            db.object("GD_X.Parts.Body"),
            Some(ObjectInfo::Part { .. })
        ));
    }

    #[test]
    fn unusable_dlc_sets_are_skipped_not_fatal() {
        let base = parse("Startup.upk", &base_world());
        let skipped_with = |world: &World| {
            let db = build(&[base.clone(), parse("Dlc.upk", world)]);
            assert_eq!(
                db.sets.keys().copied().collect::<Vec<_>>(),
                [0],
                "{:?}",
                db.skipped_packages
            );
            assert_eq!(db.skipped_packages.len(), 1);
            assert!(
                db.skipped_packages[0].starts_with("Dlc.upk: "),
                "{:?}",
                db.skipped_packages
            );
            // The base game is unaffected, and nothing of the DLC leaks into the objects.
            assert_eq!(db.objects.len(), 18);
            db.skipped_packages[0].clone()
        };

        // Package ids that cannot be a saved set id.
        for id in [0, 255, 256, -1] {
            assert!(
                skipped_with(&dlc_world(id)).contains("bad PackageId"),
                "{id}"
            );
        }
        // No `PackageId` at all, and no package definition.
        let mut w = World::new();
        let library = w.library("GD_N.AL", None, &[]);
        dlc_set(&mut w, "N", None, [library; 6]);
        assert!(skipped_with(&w).contains("bad PackageId"));
        let mut w = World::new();
        w.object("GD_N.Set", "DownloadableAssetLibraryDefinition", &[]);
        assert!(skipped_with(&w).contains("no PackageDef"));

        // Fewer than six libraries.
        let mut w = World::new();
        let library = w.library("GD_N.AL", None, &[]);
        let set = dlc_set(&mut w, "N", Some(5), [library; 6]);
        let package_def = w.get("GD_NPackageDef.PackageDef_N");
        w.pkg.set_export_props(
            set,
            &[
                p("PackageDef", V::Obj(package_def)),
                P("Libraries", 0, V::Obj(library)),
            ],
        );
        assert!(skipped_with(&w).contains("no Libraries[1]"));

        // Libraries in another order: the parts sublibrary sits in the weapon-types slot.
        let mut w = World::new();
        let parts = w.sublibrary("GD_N.Sub", Some(LIBRARY_TYPES[1]), &[&["Parts", "Body"]]);
        let wrong = w.library("GD_N.AL_A", None, &[("GD_N.Sub", parts)]);
        let empty = w.library("GD_N.AL_B", None, &[]);
        dlc_set(
            &mut w,
            "N",
            Some(5),
            [wrong, empty, empty, empty, empty, empty],
        );
        assert!(skipped_with(&w).contains("WeaponPartDefinition, not WeaponTypeDefinition"));

        // A subobject mask in a DLC sublibrary.
        let mut w = dlc_world(2);
        let sub = w.get("GD_Tulip.Sub_Bal");
        w.pkg.set_export_props(
            sub,
            &[p(
                "AssetPaths",
                V::Structs(vec![asset_path(&["Bal", "Gun"], 2)]),
            )],
        );
        assert!(skipped_with(&w).contains("nonzero IsSubobjectMask 2"));
    }

    #[test]
    fn duplicate_sets_keep_the_first() {
        let base = parse("Startup.upk", &base_world());
        // An identical copy in a later package is ignored quietly.
        let db = build(&[
            base.clone(),
            parse("A.upk", &dlc_world(2)),
            parse("B.upk", &dlc_world(2)),
        ]);
        assert_eq!(db.sets.keys().copied().collect::<Vec<_>>(), [0, 2]);
        assert!(db.skipped_packages.is_empty());

        // A different set under the same id is reported.
        let mut other = dlc_world(2);
        let sub = other.get("GD_Tulip.Sub_Bal");
        other
            .pkg
            .set_export_props(sub, &[p("AssetPaths", V::Structs(vec![]))]);
        let db = build(&[base, parse("A.upk", &dlc_world(2)), parse("B.upk", &other)]);
        assert_eq!(db.sets[&2].tables(AssetCategory::Balances)[0].len(), 1);
        assert_eq!(
            db.skipped_packages,
            ["B.upk: a different set 2 is already defined"]
        );
    }

    #[test]
    fn earlier_package_wins_a_shared_path() {
        // The DLC package carries its own copy of a base part, with another name.
        let mut w = dlc_world(2);
        w.object(
            "GD_Weap.Parts.Title_Shot",
            "WeaponNamePartDefinition",
            &[p("PartName", V::Str("Changed"))],
        );
        let db = build(&[parse("Startup.upk", &base_world()), parse("Tulip.upk", &w)]);
        assert!(matches!(
            db.object("GD_Weap.Parts.Title_Shot"),
            Some(ObjectInfo::Part { part_name: Some(name), .. }) if name == "Shot"
        ));
    }

    #[test]
    fn dlc_references_into_the_base_game_resolve() {
        // A DLC part whose rarity attribute is an import defined by the base package.
        let mut w = dlc_world(2);
        let legendary = w.external(
            "GD_Balance.Rarity.Legendary",
            "InventoryAttributeDefinition",
        );
        let missing = w.external("GD_Balance.Rarity.Nowhere", "InventoryAttributeDefinition");
        let body = w.get("GD_Tulip.Parts.Body500");
        w.pkg.set_export_props(
            body,
            &[p(
                "Rarity",
                V::Struct(vec![p("BaseValueAttribute", V::Obj(legendary))]),
            )],
        );
        let title = w.get("GD_Tulip.Parts.Title_Pearl");
        w.pkg.set_export_props(
            title,
            &[p(
                "Rarity",
                V::Struct(vec![p("BaseValueAttribute", V::Obj(missing))]),
            )],
        );
        let db = build(&[parse("Startup.upk", &base_world()), parse("Tulip.upk", &w)]);
        assert_eq!(
            db.object("GD_Tulip.Parts.Body500").unwrap().rarity(),
            Some(RarityValue::Value(5.0))
        );
        assert_eq!(
            db.object("GD_Tulip.Parts.Title_Pearl").unwrap().rarity(),
            Some(RarityValue::Unresolvable)
        );
    }

    /// Exports: 1 `Attr` (chain [2]), 2 `Res` (constant 3), 3 `Attr2` (chain [2, 2]),
    /// 4 `Attr3` (chain [1]: not a resolver), 5 `Attr4` (chain [6]), 6 `ResEmpty` (no constant).
    #[test]
    fn rarity_value_rules() {
        let mut b = Pkg::new();
        let attribute = b.class("AttributeDefinition");
        let resolver = b.class("ConstantAttributeValueResolver");
        let missing = b.class("NotInAnyPackage");
        let chain = |refs: &[i32]| [p("ValueResolverChain", V::Objs(refs.to_vec()))];
        b.export(attribute, 0, "Attr", &chain(&[2]));
        b.export(resolver, 1, "Res", &[p("ConstantValue", V::Float(3.0))]);
        b.export(attribute, 0, "Attr2", &chain(&[2, 2]));
        b.export(attribute, 0, "Attr3", &chain(&[1]));
        b.export(attribute, 0, "Attr4", &chain(&[6]));
        b.export(resolver, 5, "ResEmpty", &[]);
        let packages = [UPackage::parse("t", b.build()).unwrap()];
        let ctx = BuildContext::new(&packages);

        // Builds the struct through a real package so the values are parsed, not hand-made.
        let rarity = |fields: &[(&'static str, V)]| {
            let mut holder = Pkg::new();
            let props: Vec<P> = fields
                .iter()
                .map(|(name, value)| p(name, value.clone()))
                .collect();
            holder.export(0, 0, "Holder", &[p("Rarity", V::Struct(props))]);
            let parsed = UPackage::parse("h", holder.build()).unwrap();
            let list = parsed.properties_of_export(0).unwrap();
            ctx.rarity(list.get("Rarity"), 0)
        };
        use RarityValue::{Unresolvable, Value};

        assert_eq!(ctx.rarity(None, 0), RarityValue::None);
        assert_eq!(rarity(&[]), Value(0.0));
        assert_eq!(
            rarity(&[("BaseValueConstant", V::Float(502.0))]),
            Value(502.0)
        );
        assert_eq!(rarity(&[("BaseValueAttribute", V::Obj(1))]), Value(3.0));
        // An attribute replaces the constant; it is not added to it.
        assert_eq!(
            rarity(&[
                ("BaseValueConstant", V::Float(500.0)),
                ("BaseValueAttribute", V::Obj(1))
            ]),
            Value(3.0)
        );
        assert_eq!(
            rarity(&[
                ("BaseValueConstant", V::Float(2.0)),
                ("BaseValueAttribute", V::Obj(0))
            ]),
            Value(2.0)
        );
        assert_eq!(
            rarity(&[
                ("BaseValueAttribute", V::Obj(1)),
                ("BaseValueScaleConstant", V::Float(1.0))
            ]),
            Value(3.0)
        );
        assert_eq!(
            rarity(&[
                ("BaseValueAttribute", V::Obj(1)),
                ("BaseValueScaleConstant", V::Float(2.0))
            ]),
            Unresolvable
        );
        assert_eq!(
            rarity(&[
                ("BaseValueAttribute", V::Obj(1)),
                ("InitializationDefinition", V::Obj(2))
            ]),
            Unresolvable
        );
        assert_eq!(
            rarity(&[
                ("BaseValueAttribute", V::Obj(1)),
                ("InitializationDefinition", V::Obj(0))
            ]),
            Value(3.0)
        );
        assert_eq!(rarity(&[("BaseValueAttribute", V::Obj(3))]), Unresolvable); // two resolvers
        assert_eq!(rarity(&[("BaseValueAttribute", V::Obj(4))]), Unresolvable); // wrong resolver class
        assert_eq!(rarity(&[("BaseValueAttribute", V::Obj(5))]), Unresolvable); // no ConstantValue
        assert_eq!(rarity(&[("BaseValueAttribute", V::Obj(99))]), Unresolvable); // bad reference
        assert_eq!(
            rarity(&[("BaseValueAttribute", V::Obj(missing))]),
            Unresolvable
        ); // not located
        assert_eq!(
            rarity(&[("BaseValueConstant", V::Float(f32::NAN))]),
            Unresolvable
        );
        assert_eq!(rarity(&[("BaseValueConstant", V::Int(5))]), Unresolvable); // wrong type
                                                                               // Not a tagged struct at all.
        let raw = PropertyValue::StructRaw {
            name: "AttributeInitializationData".into(),
            bytes: vec![1, 2],
        };
        assert_eq!(ctx.rarity(Some(&raw), 0), Unresolvable);
        let _ = rarity_constant(1.0);
    }

    #[test]
    fn weapon_kinds() {
        let kind = |name: &str| weapon_kind(Some(&PropertyValue::EnumName(name.into())));
        assert_eq!(weapon_kind(None), Some(WeaponKind::Pistol));
        assert_eq!(kind("WT_Shotgun"), Some(WeaponKind::Shotgun));
        assert_eq!(kind("WT_SMG"), Some(WeaponKind::Smg));
        assert_eq!(kind("WT_AssaultRifle"), Some(WeaponKind::AssaultRifle));
        assert_eq!(kind("WT_SniperRifle"), Some(WeaponKind::SniperRifle));
        assert_eq!(kind("WT_RocketLauncher"), Some(WeaponKind::Launcher));
        assert_eq!(kind("WT_Laser"), None);
        assert_eq!(weapon_kind(Some(&PropertyValue::Int(1))), None);
    }
}
