//! From a saved weapon's numbers to the pieces of mesh that make up its model.
//!
//! The chain, all inside the base game's `Startup.upk` (see the research document, §2):
//! the weapon type definition's `GestaltMesh` names a `GestaltSkeletalMeshDefinition`; each
//! part definition with `bIsGestaltMode` names one fragment of that mesh in
//! `GestaltModeSkeletalMeshName` and up to two more in `AdditionalGestaltModeSkeletalMeshNames`.
//! The material part names the skin.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gamedata::package::{PropertyList, UPackage};
use gamedata::{build_database, AssetCategory, ItemDatabase, ItemRefs};

use crate::error::{MeshError, Result};
use crate::gestalt::GestaltMesh;
use crate::material::MaterialParameters;
use crate::texture::Texture;

/// The names of the weapon part slots, in the order a save stores them.
pub const WEAPON_SLOTS: [&str; 9] =
    ["body", "grip", "barrel", "sight", "stock", "elemental", "accessory 1", "accessory 2", "material"];
/// The slot holding the material (skin) part, which carries no mesh.
pub const MATERIAL_SLOT: usize = 8;
/// The definition classes whose objects name a gestalt mesh: weapon types, and the item
/// kinds built the same way (shields, grenade mods, relics). Class mods are one fixed mesh
/// and are not among them.
const TYPE_CLASSES: [&str; 4] = ["WeaponTypeDefinition", "ShieldDefinition", "GrenadeModDefinition", "ArtifactDefinition"];
/// Class mods: one fixed mesh each (`NonCompositeSkeletalMesh`) with an `OverrideMaterial`.
const CLASS_MOD_CLASS: &str = "ClassModDefinition";
/// The one fragment a class mod's whole mesh is treated as.
pub const WHOLE_MESH: &str = "WholeMesh";
/// Their part definition classes.
const PART_CLASSES: [&str; 4] =
    ["WeaponPartDefinition", "ShieldPartDefinition", "GrenadeModPartDefinition", "ArtifactPartDefinition"];
/// Elements in the static array `AdditionalGestaltModeSkeletalMeshNames`.
const ADDITIONAL_NAMES: usize = 4;

/// One part slot of an assembled weapon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyPart {
    /// Index into [`WEAPON_SLOTS`].
    pub slot: usize,
    /// The part definition's path, such as `GD_Weap_AssaultRifle.Barrel.AR_Barrel_Dahl`.
    pub definition: String,
    /// The mesh fragments this part shows. Empty when the part is not in gestalt mode (the
    /// "none" sights, stocks and accessories).
    pub fragments: Vec<String>,
}

/// What a weapon's model is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// The weapon type definition's path.
    pub weapon_type: String,
    /// The gestalt definition's path, a key for [`WeaponSource::gestalt`].
    pub gestalt: String,
    pub parts: Vec<AssemblyPart>,
    /// The material instance the material part names, if any.
    pub material: Option<String>,
    /// Part slots whose value names no part definition in the package.
    pub unresolved_slots: Vec<usize>,
    /// A shield, grenade mod or relic rather than a weapon. Their models face the other way.
    pub is_item: bool,
}

impl Assembly {
    /// Every fragment name with the slot it belongs to, in slot order.
    pub fn fragments(&self) -> impl Iterator<Item = (usize, &str)> {
        self.parts.iter().flat_map(|p| p.fragments.iter().map(move |f| (p.slot, f.as_str())))
    }
}

/// What a part definition contributes to a weapon's look.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartLook {
    pub fragments: Vec<String>,
    /// The material instance a material part names.
    pub material: Option<String>,
}

impl PartLook {
    /// Reads a part definition's properties. `pkg` resolves the material reference.
    pub fn read(pkg: &UPackage, part: &PropertyList) -> Self {
        let material = part.object("Material").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty());
        Self { fragments: fragment_names(part), material }
    }
}

/// Everything needed to assemble and skin a base-game weapon, taken out of `Startup.upk` once.
/// The package itself (60 MB) is not kept.
pub struct WeaponSource {
    database: ItemDatabase,
    /// The folder holding the package and its `.tfc` texture caches.
    package_dir: PathBuf,
    /// Weapon type (or shield, grenade mod or relic definition) path to gestalt definition path.
    types: HashMap<String, String>,
    /// Which of `types` are items rather than weapons.
    item_types: HashSet<String>,
    /// Class mod definitions (also in `types`, keyed to their mesh's path) and the material
    /// each overrides its mesh's with.
    class_mods: HashMap<String, Option<String>>,
    parts: HashMap<String, PartLook>,
    gestalts: HashMap<String, Result<GestaltMesh>>,
    materials: HashMap<String, MaterialParameters>,
    /// Mip tables of the textures the materials name as `p_Masks`, `p_Diffuse`, `p_Pattern`
    /// and `p_Decal`.
    textures: HashMap<String, Texture>,
    /// Definitions that could not be read, for diagnosis.
    problems: Vec<String>,
}

impl WeaponSource {
    /// Opens `Startup.upk` (read-only), builds the base-game item tables and takes out every
    /// weapon part, weapon mesh and weapon material.
    pub fn open(startup_package: &Path) -> Result<Self> {
        let dir = startup_package.parent().unwrap_or(Path::new("")).to_path_buf();
        Self::from_package(UPackage::open(startup_package)?, dir)
    }

    pub fn from_package(package: UPackage, package_dir: PathBuf) -> Result<Self> {
        let packages = [package];
        let database = build_database(&packages, "gamemesh")?;
        let pkg = &packages[0];
        let mut problems = Vec::new();

        let mut types = HashMap::new();
        let mut item_types = HashSet::new();
        for class in TYPE_CLASSES {
            for &index in pkg.exports_of_class(class) {
                let found = pkg.export_path(index).and_then(|path| {
                    let gestalt =
                        pkg.properties_of_export(index)?.object("GestaltMesh").map(|r| pkg.path_of(r)).transpose()?;
                    Ok((path, gestalt))
                });
                match found {
                    Ok((path, Some(gestalt))) if !gestalt.is_empty() => {
                        if class != TYPE_CLASSES[0] {
                            item_types.insert(path.clone());
                        }
                        types.insert(path, gestalt);
                    }
                    // An item kind without one is simply not drawn; a weapon type should have one.
                    Ok((path, _)) if class == TYPE_CLASSES[0] => problems.push(format!("{path}: no GestaltMesh")),
                    Ok(_) => {}
                    Err(e) => problems.push(format!("{class} export {index}: {e}")),
                }
            }
        }

        // Class mods: the whole mesh is one fragment, so it is held like a gestalt mesh.
        let mut class_mods = HashMap::new();
        let mut whole_meshes = HashMap::new();
        for &index in pkg.exports_of_class(CLASS_MOD_CLASS) {
            let found = pkg.export_path(index).and_then(|path| {
                let props = pkg.properties_of_export(index)?;
                let mesh = props.object("NonCompositeSkeletalMesh").filter(|&r| r > 0);
                let material = props.object("OverrideMaterial").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty());
                Ok((path, mesh, material))
            });
            match found {
                Ok((path, Some(mesh_ref), material)) => {
                    let mesh_index = (mesh_ref - 1) as usize;
                    let Ok(mesh_path) = pkg.export_path(mesh_index) else { continue };
                    whole_meshes.entry(mesh_path.clone()).or_insert(mesh_index);
                    item_types.insert(path.clone());
                    types.insert(path.clone(), mesh_path);
                    class_mods.insert(path, material);
                }
                // A class mod whose mesh is in another package is not drawn.
                Ok(_) => {}
                Err(e) => problems.push(format!("{CLASS_MOD_CLASS} export {index}: {e}")),
            }
        }

        let mut gestalts = HashMap::new();
        for (path, &index) in &whole_meshes {
            gestalts.insert(path.clone(), GestaltMesh::whole(pkg, index, WHOLE_MESH));
        }
        for path in types.values() {
            if !gestalts.contains_key(path) {
                let mesh = pkg
                    .export_index(path)
                    .ok_or_else(|| MeshError::Missing(format!("{path} is not in {}", pkg.name)))
                    .and_then(|index| GestaltMesh::from_export(pkg, index));
                if let Err(e) = &mesh {
                    problems.push(format!("{path}: {e}"));
                }
                gestalts.insert(path.clone(), mesh);
            }
        }

        let mut parts = HashMap::new();
        for class in PART_CLASSES {
            for &index in pkg.exports_of_class(class) {
                match pkg.export_path(index).and_then(|path| Ok((path, pkg.properties_of_export(index)?))) {
                    Ok((path, props)) => {
                        parts.insert(path, PartLook::read(pkg, &props));
                    }
                    Err(e) => problems.push(format!("{class} export {index}: {e}")),
                }
            }
        }

        let mut materials = HashMap::new();
        let mut textures = HashMap::new();
        let wanted = parts.values().filter_map(|look: &PartLook| look.material.as_ref()).chain(class_mods.values().flatten());
        for material in wanted {
            if materials.contains_key(material) {
                continue;
            }
            match MaterialParameters::read(pkg, material) {
                Ok(params) => {
                    for name in ["p_Masks", "p_Diffuse", "p_Pattern", "p_Decal"] {
                        let Some(path) = params.texture(name).filter(|p| !p.is_empty()) else { continue };
                        if textures.contains_key(path) {
                            continue;
                        }
                        let texture = pkg
                            .export_index(path)
                            .ok_or_else(|| MeshError::Missing(format!("texture {path}")))
                            .and_then(|index| Texture::from_export(pkg, index));
                        match texture {
                            Ok(texture) => {
                                textures.insert(path.to_string(), texture);
                            }
                            Err(e) => problems.push(format!("{path}: {e}")),
                        }
                    }
                    materials.insert(material.clone(), params);
                }
                Err(e) => problems.push(format!("{material}: {e}")),
            }
        }

        Ok(Self { database, package_dir, types, item_types, class_mods, parts, gestalts, materials, textures, problems })
    }

    pub fn database(&self) -> &ItemDatabase {
        &self.database
    }

    /// The folder the texture caches are read from.
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    /// Definitions that could not be read when the source was opened. Empty on a sound install.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// The paths of every weapon type definition.
    pub fn weapon_types(&self) -> impl Iterator<Item = &str> {
        self.types.keys().map(String::as_str).filter(|path| !self.item_types.contains(*path))
    }

    /// The shield, grenade mod and relic definitions that are built from parts.
    pub fn item_types(&self) -> impl Iterator<Item = &str> {
        self.item_types.iter().map(String::as_str)
    }

    /// The paths of the gestalt definitions the weapon types use.
    pub fn gestalt_paths(&self) -> impl Iterator<Item = &str> {
        self.gestalts.keys().map(String::as_str)
    }

    /// Which fragments of which gestalt mesh a saved weapon shows. Base-game weapons only:
    /// any other set is refused.
    /// The pieces of a base-game weapon, shield, grenade mod or relic. Any other kind of item
    /// (a class mod, a story item) is [`MeshError::Unsupported`]: it is not made of parts.
    pub fn assemble(&self, item: &ItemRefs) -> Result<Assembly> {
        if item.set != Some(0) {
            return Err(MeshError::Unsupported(format!("item set {:?}: only base-game items are handled", item.set)));
        }
        let (type_category, part_category) = if item.is_weapon {
            (AssetCategory::WeaponTypes, AssetCategory::WeaponParts)
        } else {
            (AssetCategory::ItemTypes, AssetCategory::ItemParts)
        };
        let type_value = item.item_type.ok_or_else(|| MeshError::Missing("the item type".into()))?;
        let weapon_type = self
            .database
            .path(type_category, type_value, 0)
            .ok_or_else(|| MeshError::Missing(format!("item type {type_value} in the base tables")))?;
        if !item.is_weapon && !self.types.contains_key(weapon_type) {
            return Err(MeshError::Unsupported(format!("{weapon_type} is not built from parts")));
        }
        // A class mod's parts change its bonuses, not its looks: it is its one mesh.
        if let Some(material) = self.class_mods.get(weapon_type) {
            return Ok(Assembly {
                weapon_type: weapon_type.to_string(),
                gestalt: self.types[weapon_type].clone(),
                parts: vec![AssemblyPart { slot: 0, definition: weapon_type.to_string(), fragments: vec![WHOLE_MESH.to_string()] }],
                material: material.clone(),
                unresolved_slots: Vec::new(),
                is_item: true,
            });
        }
        let mut parts: [Option<&str>; 9] = [None; 9];
        let mut unknown = Vec::new();
        for (slot, value) in item.parts.iter().take(WEAPON_SLOTS.len()).enumerate() {
            let Some(value) = value.filter(|&v| v != part_category.all_ones()) else {
                continue;
            };
            match self.database.path(part_category, value, 0) {
                Some(path) => parts[slot] = Some(path),
                None => unknown.push(slot),
            }
        }
        let mut assembly = self.assemble_paths(weapon_type, &parts)?;
        assembly.unresolved_slots.extend(unknown);
        assembly.unresolved_slots.sort_unstable();
        Ok(assembly)
    }

    /// As [`WeaponSource::assemble`], from definition paths: the weapon type's, and one part
    /// path per slot (`None` for an empty slot).
    pub fn assemble_paths(&self, weapon_type: &str, parts: &[Option<&str>; 9]) -> Result<Assembly> {
        let gestalt = self
            .types
            .get(weapon_type)
            .ok_or_else(|| MeshError::Missing(format!("weapon type {weapon_type}")))?
            .clone();
        let mut out = Vec::new();
        let mut material = None;
        let mut unresolved_slots = Vec::new();
        for (slot, path) in parts.iter().enumerate() {
            let Some(path) = path else { continue };
            let Some(look) = self.parts.get(*path) else {
                unresolved_slots.push(slot);
                continue;
            };
            if slot == MATERIAL_SLOT {
                material = look.material.clone();
            }
            out.push(AssemblyPart { slot, definition: (*path).to_string(), fragments: look.fragments.clone() });
        }
        let is_item = self.item_types.contains(weapon_type);
        Ok(Assembly { weapon_type: weapon_type.to_string(), gestalt, parts: out, material, unresolved_slots, is_item })
    }

    /// The gestalt mesh with this definition path.
    pub fn gestalt(&self, definition_path: &str) -> Result<&GestaltMesh> {
        match self.gestalts.get(definition_path) {
            Some(Ok(mesh)) => Ok(mesh),
            Some(Err(e)) => Err(e.clone()),
            None => Err(MeshError::Missing(format!("gestalt definition {definition_path}"))),
        }
    }

    /// The parameters of a weapon material instance.
    pub fn material(&self, path: &str) -> Option<&MaterialParameters> {
        self.materials.get(path)
    }

    /// The mip table of a texture a weapon material names.
    pub fn texture(&self, path: &str) -> Option<&Texture> {
        self.textures.get(path)
    }
}

/// The fragment names a part definition shows: none unless it is in gestalt mode.
pub fn fragment_names(part: &PropertyList) -> Vec<String> {
    if part.bool("bIsGestaltMode") != Some(true) {
        return Vec::new();
    }
    let additional = (0..ADDITIONAL_NAMES).filter_map(|i| part.name_at("AdditionalGestaltModeSkeletalMeshNames", i));
    part.name("GestaltModeSkeletalMeshName")
        .into_iter()
        .chain(additional)
        .filter(|name| *name != "None")
        .map(str::to_string)
        .collect()
}
