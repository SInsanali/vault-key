//! Test-only helpers: temporary folders, synthetic installs, and a builder for minimal valid
//! version-832 packages.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::package::PACKAGE_TAG;

// MARK: Files

/// A fresh folder under the system temp directory, removed when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let name = format!(
            "gk-gamedata-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates a folder that passes for an install: `root` with an empty but valid `Startup.upk`.
/// Returns `root`.
pub(crate) fn synthetic_install(root: &Path) -> PathBuf {
    let cooked = root.join("WillowGame").join("CookedPCConsole");
    std::fs::create_dir_all(&cooked).unwrap();
    std::fs::write(cooked.join("Startup.upk"), Pkg::new().build()).unwrap();
    root.to_path_buf()
}

/// Makes `link` a directory link to `target`: a symbolic link, or on Windows a junction when
/// symbolic links need a privilege the test run lacks. Returns false when neither works.
pub(crate) fn link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
            || std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

// MARK: Bytes

pub(crate) fn le32(v: i32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

pub(crate) fn fstring(s: &str) -> Vec<u8> {
    let mut out = le32(s.len() as i32 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    out
}

/// An FName: name-table index and number.
pub(crate) fn fname(index: i32, number: i32) -> Vec<u8> {
    [le32(index), le32(number)].concat()
}

/// A tagged property from raw parts: name and type indices, the size field (the value's
/// length unless given), the array index, extra header bytes, and the value bytes.
pub(crate) fn raw_tag(
    name: i32,
    type_name: i32,
    size: Option<i32>,
    array_index: i32,
    extra: &[u8],
    value: &[u8],
) -> Vec<u8> {
    let mut out = [fname(name, 0), fname(type_name, 0)].concat();
    out.extend(le32(size.unwrap_or(value.len() as i32)));
    out.extend(le32(array_index));
    out.extend_from_slice(extra);
    out.extend_from_slice(value);
    out
}

/// An object body: net index 0, the tags, then the `None` name (index 0).
pub(crate) fn property_body(tags: &[Vec<u8>]) -> Vec<u8> {
    let mut out = le32(0);
    out.extend(tags.concat());
    out.extend(fname(0, 0));
    out
}

// MARK: Packages

/// Header size with an empty folder string and one generation entry.
pub(crate) const HEADER_SIZE: usize = 112;
/// Where the compression flags and chunk count sit in that header.
pub(crate) const FLAGS_OFFSET: usize = 104;
/// The size of an export record with no net objects.
pub(crate) const EXPORT_SIZE: usize = 68;

/// A property value for [`Pkg::export`].
#[derive(Clone)]
pub(crate) enum V {
    Int(i32),
    Float(f32),
    Bool(bool),
    Byte(u8),
    Str(&'static str),
    Name(&'static str),
    Enum(&'static str),
    Obj(i32),
    Color([u8; 4]),
    Struct(Vec<P>),
    Objs(Vec<i32>),
    Strs(Vec<&'static str>),
    Structs(Vec<Vec<P>>),
}

/// A property: name, static-array index, value.
#[derive(Clone)]
pub(crate) struct P(pub(crate) &'static str, pub(crate) i32, pub(crate) V);

/// Shorthand for a property at static-array index 0.
pub(crate) fn p(name: &'static str, value: V) -> P {
    P(name, 0, value)
}

struct Import {
    class_package: i32,
    class_name: i32,
    outer: i32,
    name: i32,
}

struct Export {
    class_ref: i32,
    outer: i32,
    name: (i32, i32),
    archetype: i32,
    body: Vec<u8>,
}

/// Builds a minimal valid version-832, licensee-46 package. Name index 0 is always `None`.
/// [`Pkg::import`] and [`Pkg::export`] return the object reference of what they add.
pub(crate) struct Pkg {
    names: Vec<String>,
    imports: Vec<Import>,
    exports: Vec<Export>,
}

impl Pkg {
    pub(crate) fn new() -> Self {
        Self {
            names: vec!["None".into()],
            imports: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// A package whose name table is `None` followed by `names`, so index = position + 1.
    pub(crate) fn with_names(names: &[&str]) -> Self {
        let mut pkg = Self::new();
        for n in names {
            pkg.name(n);
        }
        pkg
    }

    /// The name-table index of `s`, adding it if needed.
    pub(crate) fn name(&mut self, s: &str) -> i32 {
        let index = self.names.iter().position(|n| n == s).unwrap_or_else(|| {
            self.names.push(s.to_string());
            self.names.len() - 1
        });
        index as i32
    }

    pub(crate) fn import(&mut self, class_name: &str, outer: i32, name: &str) -> i32 {
        let import = Import {
            class_package: self.name("Core"),
            class_name: self.name(class_name),
            outer,
            name: self.name(name),
        };
        self.imports.push(import);
        -(self.imports.len() as i32)
    }

    /// An import of the class `name` itself (outer null).
    pub(crate) fn class(&mut self, name: &str) -> i32 {
        self.import("Class", 0, name)
    }

    /// An export whose body is the given bytes.
    pub(crate) fn raw_export(
        &mut self,
        class_ref: i32,
        outer: i32,
        name: &str,
        body: Vec<u8>,
    ) -> i32 {
        let name = (self.name(name), 0);
        self.exports.push(Export {
            class_ref,
            outer,
            name,
            archetype: 0,
            body,
        });
        self.exports.len() as i32
    }

    /// An export whose body is the given properties.
    pub(crate) fn export(&mut self, class_ref: i32, outer: i32, name: &str, props: &[P]) -> i32 {
        let mut body = le32(0);
        body.extend(self.tags(props));
        self.raw_export(class_ref, outer, name, body)
    }

    /// Replaces the properties of the export with reference `export`.
    pub(crate) fn set_export_props(&mut self, export: i32, props: &[P]) {
        let mut body = le32(0);
        body.extend(self.tags(props));
        self.exports[export as usize - 1].body = body;
    }

    /// Sets the archetype of the export with reference `export`.
    pub(crate) fn set_export_archetype(&mut self, export: i32, archetype: i32) {
        self.exports[export as usize - 1].archetype = archetype;
    }

    /// Gives the most recent export's name a number, so it reads `<name>_<number-1>`.
    pub(crate) fn number_last_export(&mut self, number: i32) {
        self.exports.last_mut().unwrap().name.1 = number;
    }

    /// The properties followed by the `None` terminator.
    fn tags(&mut self, props: &[P]) -> Vec<u8> {
        let mut out = Vec::new();
        for P(name, index, value) in props {
            let (type_name, extra, bytes): (&str, Vec<u8>, Vec<u8>) = match value {
                V::Int(v) => ("IntProperty", vec![], le32(*v)),
                V::Float(v) => ("FloatProperty", vec![], v.to_le_bytes().to_vec()),
                V::Bool(v) => ("BoolProperty", vec![u8::from(*v)], vec![]),
                V::Byte(v) => ("ByteProperty", fname(0, 0), vec![*v]),
                V::Str(s) => ("StrProperty", vec![], fstring(s)),
                V::Name(s) => ("NameProperty", vec![], fname(self.name(s), 0)),
                V::Enum(s) => (
                    "ByteProperty",
                    fname(self.name("Enum"), 0),
                    fname(self.name(s), 0),
                ),
                V::Obj(r) => ("ObjectProperty", vec![], le32(*r)),
                V::Color(c) => ("StructProperty", fname(self.name("Color"), 0), c.to_vec()),
                V::Struct(fields) => (
                    "StructProperty",
                    fname(self.name("SomeStruct"), 0),
                    self.tags(fields),
                ),
                V::Objs(refs) => {
                    let mut b = le32(refs.len() as i32);
                    refs.iter().for_each(|r| b.extend(le32(*r)));
                    ("ArrayProperty", vec![], b)
                }
                V::Strs(strings) => {
                    let mut b = le32(strings.len() as i32);
                    strings.iter().for_each(|s| b.extend(fstring(s)));
                    ("ArrayProperty", vec![], b)
                }
                V::Structs(elements) => {
                    let mut b = le32(elements.len() as i32);
                    for element in elements {
                        b.extend(self.tags(element));
                    }
                    ("ArrayProperty", vec![], b)
                }
            };
            let (name, type_name) = (self.name(name), self.name(type_name));
            out.extend(raw_tag(name, type_name, None, *index, &extra, &bytes));
        }
        out.extend(fname(0, 0));
        out
    }

    pub(crate) fn build(&self) -> Vec<u8> {
        let mut name_bytes = Vec::new();
        for n in &self.names {
            name_bytes.extend(fstring(n));
            name_bytes.extend([0u8; 8]);
        }
        let mut import_bytes = Vec::new();
        for i in &self.imports {
            import_bytes.extend(fname(i.class_package, 0));
            import_bytes.extend(fname(i.class_name, 0));
            import_bytes.extend(le32(i.outer));
            import_bytes.extend(fname(i.name, 0));
        }
        let name_offset = HEADER_SIZE;
        let import_offset = name_offset + name_bytes.len();
        let export_offset = import_offset + import_bytes.len();
        let data_offset = export_offset + EXPORT_SIZE * self.exports.len();

        let mut table = Vec::new();
        let mut bodies = Vec::new();
        for e in &self.exports {
            let serial_offset = data_offset + bodies.len();
            table.extend(le32(e.class_ref));
            table.extend(le32(0)); // super
            table.extend(le32(e.outer));
            table.extend(fname(e.name.0, e.name.1));
            table.extend(le32(e.archetype));
            table.extend([0u8; 8]); // object flags
            table.extend(le32(e.body.len() as i32));
            table.extend(le32(serial_offset as i32));
            table.extend(le32(0)); // export flags
            table.extend(le32(0)); // net object count
            table.extend([0u8; 16]); // GUID
            table.extend(le32(0)); // package flags
            bodies.extend_from_slice(&e.body);
        }

        let mut h = PACKAGE_TAG.to_le_bytes().to_vec();
        h.extend([0x40, 0x03, 46, 0]); // version 832, licensee 46
        h.extend(le32(data_offset as i32)); // total header size
        h.extend(le32(0)); // empty folder
        h.extend(le32(0)); // package flags
        h.extend(le32(self.names.len() as i32));
        h.extend(le32(name_offset as i32));
        h.extend(le32(self.exports.len() as i32));
        h.extend(le32(export_offset as i32));
        h.extend(le32(self.imports.len() as i32));
        h.extend(le32(import_offset as i32));
        h.extend(le32(0)); // depends
        h.extend([0u8; 12]); // import/export guids
        h.extend(le32(0)); // thumbnails
        h.extend([0xAB; 16]); // GUID
        h.extend(le32(1)); // one generation
        h.extend(le32(self.exports.len() as i32));
        h.extend(le32(self.names.len() as i32));
        h.extend(le32(0));
        h.extend(le32(1_712_574)); // engine
        h.extend(le32(134)); // cooker
        h.extend(le32(0)); // compression flags
        h.extend(le32(0)); // chunk count
        assert_eq!(h.len(), HEADER_SIZE);
        [h, name_bytes, import_bytes, table, bodies].concat()
    }
}

// MARK: Compression wrappers

/// One chunk stream over `data`, with blocks of up to `block_size` bytes.
pub(crate) fn chunk_stream(data: &[u8], block_size: usize) -> Vec<u8> {
    let mut pairs = Vec::new();
    let mut blocks = Vec::new();
    for block in data.chunks(block_size) {
        let compressed = lzo1x::compress_literals(block);
        pairs.extend(le32(compressed.len() as i32));
        pairs.extend(le32(block.len() as i32));
        blocks.extend(compressed);
    }
    let mut out = PACKAGE_TAG.to_le_bytes().to_vec();
    out.extend(le32(block_size as i32));
    out.extend(le32(blocks.len() as i32));
    out.extend(le32(data.len() as i32));
    out.extend(pairs);
    out.extend(blocks);
    out
}

/// Layout (a): the whole package as back-to-back chunk streams of up to `stream_limit` bytes.
pub(crate) fn whole_file_compressed(
    package: &[u8],
    block_size: usize,
    stream_limit: usize,
) -> Vec<u8> {
    package
        .chunks(stream_limit)
        .flat_map(|part| chunk_stream(part, block_size))
        .collect()
}

/// Layout (b): the header stays uncompressed with compression flags 2 and one chunk covering
/// everything after it. Decompressing yields the input with only the flags and chunk-count
/// words (bytes `FLAGS_OFFSET..FLAGS_OFFSET + 8`) changed, because the header keeps its size.
pub(crate) fn chunked(package: &[u8], block_size: usize) -> Vec<u8> {
    let body = &package[HEADER_SIZE..];
    let stream = chunk_stream(body, block_size);
    let mut out = package[..FLAGS_OFFSET].to_vec();
    out.extend(le32(2));
    out.extend(le32(1));
    out.extend(le32(HEADER_SIZE as i32));
    out.extend(le32(body.len() as i32));
    out.extend(le32(HEADER_SIZE as i32 + 16));
    out.extend(le32(stream.len() as i32));
    out.extend(stream);
    out
}

// MARK: Game-shaped packages

/// A [`Pkg`] addressed by object path: outers are created as needed, classes are imported once.
pub(crate) struct World {
    pub(crate) pkg: Pkg,
    refs: std::collections::HashMap<String, i32>,
    classes: std::collections::HashMap<String, i32>,
}

/// The `LibraryType` class of each asset category, in category order.
pub(crate) const LIBRARY_TYPES: [&str; 6] = [
    "WeaponTypeDefinition",
    "WeaponPartDefinition",
    "ItemDefinition",
    "ItemPartDefinition",
    "ManufacturerDefinition",
    "InventoryBalanceDefinition",
];

impl World {
    pub(crate) fn new() -> Self {
        Self {
            pkg: Pkg::new(),
            refs: Default::default(),
            classes: Default::default(),
        }
    }

    /// The reference of the imported class `name`.
    pub(crate) fn class(&mut self, name: &str) -> i32 {
        if let Some(&r) = self.classes.get(name) {
            return r;
        }
        let r = self.pkg.class(name);
        self.classes.insert(name.to_string(), r);
        r
    }

    /// The reference of the object at `path`, if it was added.
    pub(crate) fn get(&self, path: &str) -> i32 {
        self.refs[path]
    }

    /// Adds an export at the dotted `path`, creating missing outers as `Package` exports.
    pub(crate) fn object(&mut self, path: &str, class: &str, props: &[P]) -> i32 {
        let (outer, name) = match path.rsplit_once('.') {
            Some((outer_path, name)) => {
                let outer = match self.refs.get(outer_path) {
                    Some(&r) => r,
                    None => self.object(outer_path, "Package", &[]),
                };
                (outer, name)
            }
            None => (0, path),
        };
        let class = self.class(class);
        let r = self.pkg.export(class, outer, name, props);
        self.refs.insert(path.to_string(), r);
        r
    }

    /// An import of an object defined in another package, by dotted path.
    pub(crate) fn external(&mut self, path: &str, class: &str) -> i32 {
        let mut outer = 0;
        let parts: Vec<&str> = path.split('.').collect();
        for (i, part) in parts.iter().enumerate() {
            let class = if i + 1 == parts.len() {
                class
            } else {
                "Package"
            };
            outer = self.pkg.import(class, outer, part);
        }
        outer
    }

    /// A `PackageAssetSublibrary` at `path`. Each asset is its path components below the
    /// sublibrary's root package; an empty list is an entry with no path.
    pub(crate) fn sublibrary(
        &mut self,
        path: &str,
        library_type: Option<&str>,
        assets: &[&[&'static str]],
    ) -> i32 {
        let entries = assets
            .iter()
            .map(|components| asset_path(components, 0))
            .collect();
        let mut props = Vec::new();
        if let Some(t) = library_type {
            props.push(p("LibraryType", V::Obj(self.class(t))));
        }
        props.push(p("AssetPaths", V::Structs(entries)));
        self.object(path, "PackageAssetSublibrary", &props)
    }

    /// A `PackageAssetLibrary` at `path` over `(name, link)` sublibraries.
    pub(crate) fn library(
        &mut self,
        path: &str,
        library_type: Option<&str>,
        sublibraries: &[(&'static str, i32)],
    ) -> i32 {
        let mut props = Vec::new();
        if let Some(t) = library_type {
            props.push(p("LibraryType", V::Obj(self.class(t))));
        }
        if !sublibraries.is_empty() {
            props.push(p(
                "Sublibraries",
                V::Strs(sublibraries.iter().map(|s| s.0).collect()),
            ));
            props.push(p(
                "SublibraryLinks",
                V::Objs(sublibraries.iter().map(|s| s.1).collect()),
            ));
        }
        self.object(path, "PackageAssetLibrary", &props)
    }

    /// `GD_Globals.General.Globals` over the six base libraries (which must exist at their
    /// real paths) and a colour table shaped like the game's.
    pub(crate) fn globals(&mut self) -> i32 {
        let libraries = [
            "WeaponTypes",
            "WeaponParts",
            "ItemTypes",
            "ItemParts",
            "Manufacturer",
            "Balance",
        ];
        let mut props: Vec<P> = libraries
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let library = self.get(&format!("GD_AssetLibraries.Inventory.AL_{name}"));
                P("AssetLibraries", i as i32, V::Obj(library))
            })
            .collect();
        // B, G, R, A as stored.
        let row = |min: i32, max: i32, bgra: [u8; 4], rating: &'static str| {
            let mut fields = vec![];
            if min != 0 {
                fields.push(p("MinLevel", V::Int(min)));
            }
            if max != 0 {
                fields.push(p("MaxLevel", V::Int(max)));
            }
            fields.push(p("Color", V::Color(bgra)));
            fields.push(p("RarityRating", V::Enum(rating)));
            fields
        };
        props.push(p(
            "RarityLevelColors",
            V::Structs(vec![
                row(0, 0, [175, 193, 205, 0], "RARITY_Unknown"),
                row(1, 1, [255, 255, 255, 255], "RARITY_Common"),
                row(2, 2, [11, 210, 61, 255], "RARITY_Uncommon"),
                row(4, 4, [229, 63, 168, 255], "RARITY_VeryRare"),
                row(6, 6, [168, 0, 202, 255], "RARITY_VeryRare"),
                row(7, 10, [0, 180, 255, 255], "RARITY_Legendary"),
                row(12, 170, [0, 0, 0, 0], "RARITY_Unknown"),
                row(500, 500, [255, 255, 0, 0], "RARITY_Unknown"),
                row(501, 501, [184, 154, 255, 0], "RARITY_Seraph"),
            ]),
        ));
        self.object("GD_Globals.General.Globals", "GlobalsDefinition", &props)
    }

    /// An attribute whose single constant resolver gives `value`.
    pub(crate) fn rarity_attribute(&mut self, path: &str, value: f32) -> i32 {
        let attribute = self.object(path, "InventoryAttributeDefinition", &[]);
        let resolver = self.object(
            &format!("{path}.ConstantAttributeValueResolver_0"),
            "ConstantAttributeValueResolver",
            &[p("ConstantValue", V::Float(value))],
        );
        // The attribute was exported before its resolver existed, so rebuild its body.
        self.pkg.set_export_props(
            attribute,
            &[p("ValueResolverChain", V::Objs(vec![resolver]))],
        );
        attribute
    }

    pub(crate) fn build(&self) -> Vec<u8> {
        self.pkg.build()
    }
}

/// One `AssetPaths` entry: the components right-aligned in the six `PathComponentNames`.
pub(crate) fn asset_path(components: &[&'static str], mask: u8) -> Vec<P> {
    let mut entry: Vec<P> = (0..6)
        .map(|i| {
            let from_end = 6 - i;
            let name = if from_end <= components.len() {
                components[components.len() - from_end]
            } else {
                "None"
            };
            P("PathComponentNames", i as i32, V::Name(name))
        })
        .collect();
    entry.push(p("IsSubobjectMask", V::Byte(mask)));
    entry
}

/// A rarity struct that names an attribute.
pub(crate) fn rarity_from(attribute: i32) -> V {
    V::Struct(vec![p("BaseValueAttribute", V::Obj(attribute))])
}

/// A rarity struct with a constant.
pub(crate) fn rarity_constant(value: f32) -> V {
    V::Struct(vec![p("BaseValueConstant", V::Float(value))])
}

/// A small base game. Set 0 afterwards holds:
///
/// | Category | Lib 0 | Lib 1 |
/// |---|---|---|
/// | weapon types | `GD_Weap.A.Pistol`, `GD_Weap.A.Sniper` | |
/// | weapon parts | `Body2`, `Body4`, `Barrel5`, `BarrelAlien` (2), `Prefix_Hot`, `Title_Shot`, `Grip` (all under `GD_Weap.Parts`), an entry with no path | |
/// | item types | `GD_Items.Types.Shield` (base rarity 1), `GD_Items.Types.Echo` (full name, 502), `GD_Items.Types.Skin` | (null link: `GD_Gone.PackageAssetSublibrary_9`) |
/// | item parts | `GD_Items.Parts.Material3`, `GD_Items.Parts.Title_Shield`, `GD_Items.Parts.NotThere` (not in the package) | |
/// | manufacturers | `GD_Manufacturers.M.Maker` | |
/// | balances | `GD_Weap.Bal.Weapon`, `GD_Weap.Bal.Mission` | `GD_Items.Bal.Shield` |
pub(crate) fn base_world() -> World {
    let mut w = World::new();
    let legendary = w.rarity_attribute("GD_Balance.Rarity.Legendary", 5.0);
    let alien = w.rarity_attribute("GD_Balance.Rarity.VeryRare_Alien", 2.0);

    w.object(
        "GD_Weap.A.Pistol",
        "WeaponTypeDefinition",
        &[p("Typename", V::Str("Pistol"))],
    );
    w.object(
        "GD_Weap.A.Sniper",
        "WeaponTypeDefinition",
        &[
            p("WeaponType", V::Enum("WT_SniperRifle")),
            p("Typename", V::Str("Sniper Rifle")),
        ],
    );
    w.object(
        "GD_Weap.Parts.Body2",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_constant(2.0))],
    );
    w.object(
        "GD_Weap.Parts.Body4",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_constant(4.0))],
    );
    w.object(
        "GD_Weap.Parts.Barrel5",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_from(legendary))],
    );
    w.object(
        "GD_Weap.Parts.BarrelAlien",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_from(alien))],
    );
    w.object(
        "GD_Weap.Parts.Prefix_Hot",
        "WeaponNamePartDefinition",
        &[p("PartName", V::Str("Hot"))],
    );
    w.object(
        "GD_Weap.Parts.Title_Shot",
        "WeaponNamePartDefinition",
        &[p("PartName", V::Str("Shot"))],
    );
    w.object("GD_Weap.Parts.Grip", "WeaponPartDefinition", &[]);
    w.object("GD_Weap.Bal.Weapon", "WeaponBalanceDefinition", &[]);
    w.object("GD_Weap.Bal.Mission", "MissionWeaponBalanceDefinition", &[]);

    w.object(
        "GD_Items.Types.Shield",
        "ShieldDefinition",
        &[
            p("ItemName", V::Str("Shield")),
            p("BaseRarity", rarity_constant(1.0)),
        ],
    );
    w.object(
        "GD_Items.Types.Echo",
        "UsableItemDefinition",
        &[
            p("ItemName", V::Str("Helmet Data")),
            p("bItemNameIsFullName", V::Bool(true)),
            p("BaseRarity", rarity_constant(502.0)),
        ],
    );
    w.object(
        "GD_Items.Types.Skin",
        "UsableCustomizationItemDefinition",
        &[p("BaseRarity", rarity_constant(2.0))],
    );
    w.object(
        "GD_Items.Parts.Material3",
        "ShieldPartDefinition",
        &[p("Rarity", rarity_constant(3.0))],
    );
    w.object(
        "GD_Items.Parts.Title_Shield",
        "ItemNamePartDefinition",
        &[p("PartName", V::Str("Turtle Shield"))],
    );
    w.object("GD_Items.Bal.Shield", "InventoryBalanceDefinition", &[]);
    w.object(
        "GD_Manufacturers.M.Maker",
        "ManufacturerDefinition",
        &[p(
            "Grades",
            V::Structs(vec![
                vec![p("DisplayName", V::Str("Maker"))],
                vec![p("DisplayName", V::Str("Other"))],
            ]),
        )],
    );

    let t = LIBRARY_TYPES;
    let weapon_types = w.sublibrary(
        "GD_Weap.Sub_Types",
        Some(t[0]),
        &[&["A", "Pistol"], &["A", "Sniper"]],
    );
    let weapon_parts = w.sublibrary(
        "GD_Weap.Sub_Parts",
        Some(t[1]),
        &[
            &["Parts", "Body2"],
            &["Parts", "Body4"],
            &["Parts", "Barrel5"],
            &["Parts", "BarrelAlien"],
            &["Parts", "Prefix_Hot"],
            &["Parts", "Title_Shot"],
            &["Parts", "Grip"],
            &[],
        ],
    );
    let item_types = w.sublibrary(
        "GD_Items.Sub_Types",
        Some(t[2]),
        &[&["Types", "Shield"], &["Types", "Echo"], &["Types", "Skin"]],
    );
    let item_parts = w.sublibrary(
        "GD_Items.Sub_Parts",
        Some(t[3]),
        &[
            &["Parts", "Material3"],
            &["Parts", "Title_Shield"],
            &["Parts", "NotThere"],
        ],
    );
    let manufacturers = w.sublibrary("GD_Manufacturers.Sub", Some(t[4]), &[&["M", "Maker"]]);
    let weapon_balances = w.sublibrary(
        "GD_Weap.Sub_Bal",
        Some(t[5]),
        &[&["Bal", "Weapon"], &["Bal", "Mission"]],
    );
    let item_balances = w.sublibrary("GD_Items.Sub_Bal", Some(t[5]), &[&["Bal", "Shield"]]);

    let al = "GD_AssetLibraries.Inventory.AL_";
    w.library(
        &format!("{al}WeaponTypes"),
        Some(t[0]),
        &[("GD_Weap.Sub_Types", weapon_types)],
    );
    w.library(
        &format!("{al}WeaponParts"),
        Some(t[1]),
        &[("GD_Weap.Sub_Parts", weapon_parts)],
    );
    w.library(
        &format!("{al}ItemTypes"),
        Some(t[2]),
        &[
            ("GD_Items.Sub_Types", item_types),
            ("GD_Gone.PackageAssetSublibrary_9", 0),
        ],
    );
    w.library(
        &format!("{al}ItemParts"),
        Some(t[3]),
        &[("GD_Items.Sub_Parts", item_parts)],
    );
    w.library(
        &format!("{al}Manufacturer"),
        Some(t[4]),
        &[("GD_Manufacturers.Sub", manufacturers)],
    );
    w.library(
        &format!("{al}Balance"),
        Some(t[5]),
        &[
            ("GD_Weap.Sub_Bal", weapon_balances),
            ("GD_Items.Sub_Bal", item_balances),
        ],
    );
    w.globals();
    w
}

/// A small DLC named `Tulip` with the given `PackageId`. Its set holds:
///
/// | Category | Lib 0 |
/// | weapon parts | `GD_Tulip.Parts.BarrelSeraph` (497 by attribute), `GD_Tulip.Parts.Body500`, `GD_Tulip.Parts.Title_Pearl` ("Pearl"), `GD_Tulip.Bal.Gun` (a balance filed among the parts) |
/// | item types | `GD_Tulip.Types.ClassMod` (`CrossDLCClassModDefinition`) |
/// | item parts | `GD_Tulip.Parts.Title_Mod` ("Necromancer") |
/// | balances | `GD_Tulip.Bal.Gun` |
///
/// The weapon-type and manufacturer libraries have no sublibraries, and only the sublibraries
/// carry a `LibraryType`, as in the real DLC packages.
pub(crate) fn dlc_world(package_id: i32) -> World {
    let mut w = World::new();
    let seraph = w.rarity_attribute("GD_Tulip.Rarity.Seraph", 497.0);
    w.object(
        "GD_Tulip.Parts.BarrelSeraph",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_from(seraph))],
    );
    w.object(
        "GD_Tulip.Parts.Body500",
        "WeaponPartDefinition",
        &[p("Rarity", rarity_constant(500.0))],
    );
    w.object(
        "GD_Tulip.Parts.Title_Pearl",
        "WeaponNamePartDefinition",
        &[p("PartName", V::Str("Pearl"))],
    );
    w.object(
        "GD_Tulip.Parts.Title_Mod",
        "ItemNamePartDefinition",
        &[p("PartName", V::Str("Necromancer"))],
    );
    w.object("GD_Tulip.Types.ClassMod", "CrossDLCClassModDefinition", &[]);
    w.object("GD_Tulip.Bal.Gun", "WeaponBalanceDefinition", &[]);

    let t = LIBRARY_TYPES;
    let weapon_parts = w.sublibrary(
        "GD_Tulip.Sub_WeaponParts",
        Some(t[1]),
        &[
            &["Parts", "BarrelSeraph"],
            &["Parts", "Body500"],
            &["Parts", "Title_Pearl"],
            &["Bal", "Gun"],
        ],
    );
    let item_types = w.sublibrary(
        "GD_Tulip.Sub_ItemTypes",
        Some(t[2]),
        &[&["Types", "ClassMod"]],
    );
    let item_parts = w.sublibrary(
        "GD_Tulip.Sub_ItemParts",
        Some(t[3]),
        &[&["Parts", "Title_Mod"]],
    );
    let balances = w.sublibrary("GD_Tulip.Sub_Bal", Some(t[5]), &[&["Bal", "Gun"]]);

    let al = "GD_Tulip_AssetLibraries.AL_Tulip_";
    let libraries = [
        w.library(&format!("{al}WeaponTypes"), None, &[]),
        w.library(
            &format!("{al}WeaponParts"),
            None,
            &[("GD_Tulip.Sub_WeaponParts", weapon_parts)],
        ),
        w.library(
            &format!("{al}ItemTypes"),
            None,
            &[("GD_Tulip.Sub_ItemTypes", item_types)],
        ),
        w.library(
            &format!("{al}ItemParts"),
            None,
            &[("GD_Tulip.Sub_ItemParts", item_parts)],
        ),
        w.library(&format!("{al}Manufacturer"), None, &[]),
        w.library(
            &format!("{al}Balance"),
            None,
            &[("GD_Tulip.Sub_Bal", balances)],
        ),
    ];
    dlc_set(&mut w, "Tulip", Some(package_id), libraries);
    w
}

/// Adds a DLC's package definition and asset-library set over `libraries`.
pub(crate) fn dlc_set(
    w: &mut World,
    name: &'static str,
    package_id: Option<i32>,
    libraries: [i32; 6],
) -> i32 {
    let mut def_props = vec![p("DLCName", V::Str(name))];
    if let Some(id) = package_id {
        def_props.insert(0, p("PackageId", V::Int(id)));
    }
    let package_def = w.object(
        &format!("GD_{name}PackageDef.PackageDef_{name}"),
        "DownloadablePackageDefinition",
        &def_props,
    );
    let mut props = vec![p("PackageDef", V::Obj(package_def))];
    props.extend(
        libraries
            .iter()
            .enumerate()
            .map(|(i, &l)| P("Libraries", i as i32, V::Obj(l))),
    );
    w.object(
        &format!("GD_{name}_AssetLibraries.{name}_AssetLibSet"),
        "DownloadableAssetLibraryDefinition",
        &props,
    )
}
