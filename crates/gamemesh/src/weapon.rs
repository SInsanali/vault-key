//! From a saved weapon's numbers to the pieces of mesh that make up its model.
//!
//! The chain, all inside the base game's `Startup.upk` (see the research document, §2):
//! the weapon type definition's `GestaltMesh` names a `GestaltSkeletalMeshDefinition`; each
//! part definition with `bIsGestaltMode` names one fragment of that mesh in
//! `GestaltModeSkeletalMeshName` and up to two more in `AdditionalGestaltModeSkeletalMeshNames`.
//! The material part names the skin.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gamedata::package::{PropertyList, PropertyValue, UPackage};
use gamedata::{build_database, AssetCategory, DlcResolution, GameInstall, ItemDatabase, ItemRefs};

use crate::error::{MeshError, Result};
use crate::gestalt::GestaltMesh;
use crate::material::{MaterialParameters, RELIC_TEXTURE};
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
#[derive(Debug, Clone, PartialEq)]
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
    /// The color a part sets the item glowing in (`p_EmissiveColor`): a relic's kind.
    pub glow: Option<[f32; 3]>,
}

impl Assembly {
    /// Every fragment name with the slot it belongs to, in slot order.
    pub fn fragments(&self) -> impl Iterator<Item = (usize, &str)> {
        self.parts.iter().flat_map(|p| p.fragments.iter().map(move |f| (p.slot, f.as_str())))
    }
}

/// What a part definition contributes to a weapon's look.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartLook {
    pub fragments: Vec<String>,
    /// The material instance a material part names.
    pub material: Option<String>,
    /// `p_EmissiveColor` from the part's `MaterialVectorParameterValues`: the color a relic
    /// body part sets its relic glowing in.
    pub glow: Option<[f32; 3]>,
}

impl PartLook {
    /// Reads a part definition's properties. `pkg` resolves the material reference.
    pub fn read(pkg: &UPackage, part: &PropertyList) -> Self {
        let material = part.object("Material").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty());
        Self { fragments: fragment_names(part), material, glow: glow_of(pkg, part) }
    }
}

/// A package to read pictures from, and the folder its `.tfc` texture caches are in.
pub struct SourcePackage {
    pub package: UPackage,
    pub texture_dir: PathBuf,
}

/// Finds objects by path across several packages, the base game's first. Seek-free DLC
/// packages refer to base-game objects by import, and sometimes carry copies of them; the
/// base game's own copy wins.
pub(crate) struct Packages<'a>(pub(crate) &'a [UPackage]);

impl Packages<'_> {
    pub(crate) fn get(&self, at: usize) -> &UPackage {
        &self.0[at]
    }

    /// The package and export index of the object with this path.
    pub(crate) fn locate(&self, path: &str) -> Option<(usize, usize)> {
        self.0.iter().enumerate().find_map(|(at, p)| p.export_index(path).map(|index| (at, index)))
    }

    /// Where a reference made in package `at` points: an export of that package, or an import
    /// found by its path in any of them.
    pub(crate) fn resolve(&self, at: usize, reference: i32) -> Option<(usize, usize)> {
        match reference {
            r if r > 0 => Some((at, (r - 1) as usize)),
            r if r < 0 => self.locate(&self.get(at).path_of(r).ok()?),
            _ => None,
        }
    }
}

/// Everything needed to assemble and skin a weapon or item, taken out of the game's startup
/// packages once: the base game's `Startup.upk` and each DLC's `<Name>_Startup_SF.upk`. The
/// packages themselves are not kept.
pub struct WeaponSource {
    database: ItemDatabase,
    /// The folder holding the base package and its `.tfc` texture caches.
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
    /// and `p_Decal`, with the folder of the package each came from.
    textures: HashMap<String, (Texture, PathBuf)>,
    /// Definitions that could not be read, for diagnosis.
    problems: Vec<String>,
}

impl WeaponSource {
    /// Opens the base game's `Startup.upk` alone (read-only): base-game items only.
    pub fn open(startup_package: &Path) -> Result<Self> {
        let dir = startup_package.parent().unwrap_or(Path::new("")).to_path_buf();
        Self::from_package(UPackage::open(startup_package)?, dir)
    }

    /// Opens the base game's startup package and every DLC startup package of an install
    /// (read-only). A DLC package that can't be read is left out and noted in
    /// [`WeaponSource::problems`]; only the base package is required.
    pub fn open_install(install: &GameInstall) -> Result<Self> {
        let startup = install.startup_package();
        let mut packages = vec![SourcePackage {
            package: UPackage::open(&startup)?,
            texture_dir: startup.parent().unwrap_or(Path::new("")).to_path_buf(),
        }];
        let mut problems = Vec::new();
        for path in install.dlc_startup_packages() {
            match UPackage::open(&path) {
                // DLC/<Name>/Compat/Content/<Name>_Startup_SF.upk keeps its texture caches
                // in DLC/<Name>/Compat/Textures.
                Ok(package) => {
                    let compat = path.parent().and_then(Path::parent).unwrap_or(Path::new(""));
                    packages.push(SourcePackage { package, texture_dir: compat.join("Textures") });
                }
                Err(e) => problems.push(format!("{}: {e}", path.display())),
            }
        }
        let mut source = Self::from_packages(packages)?;
        source.problems.splice(0..0, problems);
        Ok(source)
    }

    pub fn from_package(package: UPackage, package_dir: PathBuf) -> Result<Self> {
        Self::from_packages(vec![SourcePackage { package, texture_dir: package_dir }])
    }

    /// Builds the tables from `packages`, the base game's first.
    pub fn from_packages(packages: Vec<SourcePackage>) -> Result<Self> {
        let (packages, dirs): (Vec<UPackage>, Vec<PathBuf>) = packages.into_iter().map(|p| (p.package, p.texture_dir)).unzip();
        let package_dir = dirs.first().cloned().unwrap_or_default();
        // The item tables come from all of them, as the name database's do.
        let database = build_database(&packages, "gamemesh")?;
        let all = Packages(&packages);
        let mut problems = Vec::new();

        let mut types = HashMap::new();
        let mut item_types = HashSet::new();
        for (at, pkg) in packages.iter().enumerate() {
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
                            types.entry(path).or_insert(gestalt);
                        }
                        // An item kind without one is simply not drawn; a weapon type should have one.
                        Ok((path, _)) if class == TYPE_CLASSES[0] && at == 0 => problems.push(format!("{path}: no GestaltMesh")),
                        Ok(_) => {}
                        Err(e) => problems.push(format!("{} {class} export {index}: {e}", pkg.name)),
                    }
                }
            }
        }

        // Class mods: the whole mesh is one fragment, so it is held like a gestalt mesh.
        let mut class_mods = HashMap::new();
        let mut whole_meshes: HashMap<String, (usize, usize)> = HashMap::new();
        for (at, pkg) in packages.iter().enumerate() {
            for &index in pkg.exports_of_class(CLASS_MOD_CLASS) {
                let found = pkg.export_path(index).and_then(|path| {
                    let props = pkg.properties_of_export(index)?;
                    let mesh = props.object("NonCompositeSkeletalMesh").and_then(|r| all.resolve(at, r));
                    let material = props.object("OverrideMaterial").and_then(|r| pkg.path_of(r).ok()).filter(|p| !p.is_empty());
                    Ok((path, mesh, material))
                });
                match found {
                    Ok((path, Some((mesh_at, mesh_index)), material)) => {
                        let Ok(mesh_path) = all.get(mesh_at).export_path(mesh_index) else { continue };
                        if types.contains_key(&path) {
                            continue;
                        }
                        whole_meshes.entry(mesh_path.clone()).or_insert((mesh_at, mesh_index));
                        item_types.insert(path.clone());
                        types.insert(path.clone(), mesh_path);
                        class_mods.insert(path, material);
                    }
                    // A class mod whose mesh is in none of the packages is not drawn.
                    Ok(_) => {}
                    Err(e) => problems.push(format!("{} {CLASS_MOD_CLASS} export {index}: {e}", pkg.name)),
                }
            }
        }

        let mut gestalts = HashMap::new();
        for (path, &(at, index)) in &whole_meshes {
            gestalts.insert(path.clone(), GestaltMesh::whole(all.get(at), index, WHOLE_MESH));
        }
        for path in types.values() {
            if !gestalts.contains_key(path) {
                let mesh = all
                    .locate(path)
                    .ok_or_else(|| MeshError::Missing(format!("{path} is in none of the packages read")))
                    .and_then(|(at, index)| GestaltMesh::from_export(all.get(at), index));
                if let Err(e) = &mesh {
                    problems.push(format!("{path}: {e}"));
                }
                gestalts.insert(path.clone(), mesh);
            }
        }

        let mut parts = HashMap::new();
        for pkg in &packages {
            for class in PART_CLASSES {
                for &index in pkg.exports_of_class(class) {
                    match pkg.export_path(index).and_then(|path| Ok((path, pkg.properties_of_export(index)?))) {
                        Ok((path, props)) => {
                            parts.entry(path).or_insert_with(|| PartLook::read(pkg, &props));
                        }
                        Err(e) => problems.push(format!("{} {class} export {index}: {e}", pkg.name)),
                    }
                }
            }
        }

        let mut materials = HashMap::new();
        let mut textures = HashMap::new();
        // A class mod without an override shows its mesh's own material.
        for (class_mod, material) in class_mods.iter_mut().filter(|(_, m)| m.is_none()) {
            let Some(&(at, _)) = types.get(class_mod).and_then(|mesh| whole_meshes.get(mesh)) else { continue };
            let Some(Ok(mesh)) = types.get(class_mod).and_then(|mesh| gestalts.get(mesh)) else { continue };
            *material = mesh.mesh.materials.first().and_then(|&r| all.get(at).path_of(r).ok()).filter(|p| !p.is_empty());
        }
        let wanted: Vec<String> =
            parts.values().filter_map(|look: &PartLook| look.material.clone()).chain(class_mods.values().flatten().cloned()).collect();
        for material in wanted {
            if materials.contains_key(&material) {
                continue;
            }
            match MaterialParameters::read_across(&packages, &material) {
                Ok(params) => {
                    for name in ["p_Masks", "p_Diffuse", "p_Pattern", "p_CustomPattern", "p_Decal"] {
                        let Some(path) = params.texture(name).filter(|p| !p.is_empty()) else { continue };
                        if !textures.contains_key(path) {
                            load_texture(&all, &dirs, path, &mut textures, &mut problems);
                        }
                    }
                    materials.insert(material.clone(), params);
                }
                Err(e) => problems.push(format!("{material}: {e}")),
            }
        }
        // The relic texture is named by no material instance (see RELIC_TEXTURE).
        if all.locate(RELIC_TEXTURE).is_some() {
            load_texture(&all, &dirs, RELIC_TEXTURE, &mut textures, &mut problems);
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
    /// The pieces of a weapon, shield, grenade mod or relic, or the one mesh of a class mod.
    /// Items of a DLC set are looked up the way their names are ([`DlcResolution::Verified`]:
    /// a DLC flag on a value selects that DLC's tables). Any other kind of item (a story item,
    /// a skin) is [`MeshError::Unsupported`]: it has no model.
    pub fn assemble(&self, item: &ItemRefs) -> Result<Assembly> {
        let set = item.set.ok_or_else(|| MeshError::Missing("the item set".into()))?;
        let path = |category: AssetCategory, value: u32| self.database.path_with(category, value, set, DlcResolution::Verified);
        let (type_category, part_category) = if item.is_weapon {
            (AssetCategory::WeaponTypes, AssetCategory::WeaponParts)
        } else {
            (AssetCategory::ItemTypes, AssetCategory::ItemParts)
        };
        let type_value = item.item_type.ok_or_else(|| MeshError::Missing("the item type".into()))?;
        let weapon_type = path(type_category, type_value)
            .ok_or_else(|| MeshError::Missing(format!("item type {type_value} of set {set} in the item tables")))?;
        if !item.is_weapon && !self.types.contains_key(weapon_type) {
            return Err(MeshError::Unsupported(format!("{weapon_type} is not built from parts")));
        }
        // A class mod's parts change its bonuses, not its looks: it is its one mesh.
        if self.class_mods.contains_key(weapon_type) {
            return self.assemble_paths(weapon_type, &[None; 9]);
        }
        let mut parts: [Option<&str>; 9] = [None; 9];
        let mut unknown = Vec::new();
        for (slot, value) in item.parts.iter().take(WEAPON_SLOTS.len()).enumerate() {
            let Some(value) = value.filter(|&v| v != part_category.all_ones()) else {
                continue;
            };
            match path(part_category, value) {
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
    /// A class mod is its one mesh whatever `parts` says.
    pub fn assemble_paths(&self, weapon_type: &str, parts: &[Option<&str>; 9]) -> Result<Assembly> {
        if let Some(material) = self.class_mods.get(weapon_type) {
            return Ok(Assembly {
                weapon_type: weapon_type.to_string(),
                gestalt: self.types[weapon_type].clone(),
                parts: vec![AssemblyPart { slot: 0, definition: weapon_type.to_string(), fragments: vec![WHOLE_MESH.to_string()] }],
                material: material.clone(),
                unresolved_slots: Vec::new(),
                is_item: true,
                glow: None,
            });
        }
        let gestalt = self
            .types
            .get(weapon_type)
            .ok_or_else(|| MeshError::Missing(format!("weapon type {weapon_type}")))?
            .clone();
        let mut out = Vec::new();
        let mut material = None;
        let mut glow = None;
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
            glow = glow.or(look.glow);
            out.push(AssemblyPart { slot, definition: (*path).to_string(), fragments: look.fragments.clone() });
        }
        let is_item = self.item_types.contains(weapon_type);
        Ok(Assembly { weapon_type: weapon_type.to_string(), gestalt, parts: out, material, unresolved_slots, is_item, glow })
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
        self.textures.get(path).map(|(texture, _)| texture)
    }

    /// The folders to look for a texture's `.tfc` cache in: its own package's, then the base
    /// game's.
    pub fn texture_dirs(&self, path: &str) -> Vec<&Path> {
        let own = self.textures.get(path).map(|(_, dir)| dir.as_path());
        own.into_iter().chain([self.package_dir.as_path()]).collect()
    }
}

/// `p_EmissiveColor` among a part's `MaterialVectorParameterValues`, if it sets one.
fn glow_of(pkg: &UPackage, part: &PropertyList) -> Option<[f32; 3]> {
    let values = pkg.struct_array(part.get("MaterialVectorParameterValues")?).ok()?;
    values.iter().find(|v| v.name("ParameterName").is_some_and(|n| n.eq_ignore_ascii_case("p_EmissiveColor"))).and_then(|v| {
        match v.get("ParameterValue") {
            Some(PropertyValue::StructRaw { bytes, .. }) if bytes.len() == 16 => {
                let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                Some([f(0), f(4), f(8)]).filter(|c| c.iter().all(|v| v.is_finite()))
            }
            _ => None,
        }
    })
}

/// Reads the texture at `path` from whichever package holds it, keeping that package's
/// texture folder with it.
fn load_texture(
    all: &Packages,
    dirs: &[PathBuf],
    path: &str,
    textures: &mut HashMap<String, (Texture, PathBuf)>,
    problems: &mut Vec<String>,
) {
    let texture = all
        .locate(path)
        .ok_or_else(|| MeshError::Missing(format!("texture {path}")))
        .and_then(|(at, index)| Ok((Texture::from_export(all.get(at), index)?, dirs[at].clone())));
    match texture {
        Ok(texture) => {
            textures.insert(path.to_string(), texture);
        }
        Err(e) => problems.push(format!("{path}: {e}")),
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
