use std::collections::HashMap;
use std::path::Path;

use super::file::{self, PACKAGE_TAG};
use super::reader::ByteReader;
use crate::error::{GameDataError, Result};

/// The only package format understood: Borderlands 2's engine version and licensee version.
pub const SUPPORTED_VERSION: u16 = 832;
pub const SUPPORTED_LICENSEE: u16 = 46;

/// Longest outer chain followed before a reference is called bad (guards against cycles).
const MAX_OUTER_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEntry {
    pub class_package: String,
    pub class_name: String,
    pub outer: i32,
    pub object_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportEntry {
    pub class_ref: i32,
    pub super_ref: i32,
    pub outer: i32,
    pub object_name: String,
    pub archetype: i32,
    pub object_flags: u64,
    pub serial_size: usize,
    pub serial_offset: usize,
}

/// A decompressed UE3 package (version 832, licensee 46): name, import and export tables, and
/// object paths.
///
/// Object references are `i32`: above 0 is export index + 1, below 0 is −(import index + 1), and
/// 0 is null. The header's compression flags and chunk array are ignored; the tables are found
/// purely by the name, import and export offsets and counts, which refer to the uncompressed
/// layout.
#[derive(Debug, Clone)]
pub struct UPackage {
    pub name: String,
    pub version: u16,
    pub licensee: u16,
    pub names: Vec<String>,
    pub imports: Vec<ImportEntry>,
    pub exports: Vec<ExportEntry>,
    pub(crate) bytes: Vec<u8>,
    path_index: HashMap<String, usize>,
    class_index: HashMap<String, Vec<usize>>,
    class_names: Vec<String>,
}

impl UPackage {
    /// Reads and decompresses a package file (opened read-only), then parses its tables.
    pub fn open(path: &Path) -> Result<Self> {
        Self::parse(&file::file_name(path), file::read_package(path)?)
    }

    /// Parses the tables of an already decompressed package. `name` only labels errors.
    pub fn parse(name: &str, bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(GameDataError::Truncated(format!("{name}: header")));
        }
        let mut r = ByteReader::new(&bytes);
        if r.u32()? != PACKAGE_TAG {
            return Err(GameDataError::NotAPackage(name.to_string()));
        }
        let (version, licensee) = (r.u16()?, r.u16()?);
        if version != SUPPORTED_VERSION || licensee != SUPPORTED_LICENSEE {
            return Err(GameDataError::UnsupportedPackage { version, licensee });
        }
        r.skip(4)?; // totalHeaderSize
        r.fstring()?; // folder
        r.skip(4)?; // packageFlags
        let mut table = |what: &str, min_size: usize| -> Result<(usize, usize)> {
            let label = format!("{name}: {what} table");
            let count = r.i32_size(&label)?;
            let offset = r.i32_size(&label)?;
            if offset > bytes.len() || count > (bytes.len() - offset) / min_size {
                return Err(GameDataError::Truncated(label));
            }
            Ok((count, offset))
        };
        let (name_count, name_offset) = table("name", 12)?;
        let (export_count, export_offset) = table("export", 68)?;
        let (import_count, import_offset) = table("import", 28)?;

        let mut names = Vec::with_capacity(name_count);
        let mut nr = ByteReader::with_range(&bytes, name_offset, bytes.len());
        for _ in 0..name_count {
            names.push(nr.fstring()?);
            nr.skip(8)?; // flags
        }

        let mut imports = Vec::with_capacity(import_count);
        let mut ir = ByteReader::with_range(&bytes, import_offset, bytes.len());
        for _ in 0..import_count {
            let class_package = read_name(&mut ir, &names, name)?;
            let class_name = read_name(&mut ir, &names, name)?;
            let outer = ir.i32()?;
            let object_name = read_name(&mut ir, &names, name)?;
            imports.push(ImportEntry {
                class_package,
                class_name,
                outer,
                object_name,
            });
        }

        let mut exports = Vec::with_capacity(export_count);
        let mut er = ByteReader::with_range(&bytes, export_offset, bytes.len());
        for _ in 0..export_count {
            let (class_ref, super_ref, outer) = (er.i32()?, er.i32()?, er.i32()?);
            let object_name = read_name(&mut er, &names, name)?;
            let archetype = er.i32()?;
            let object_flags = er.u64()?;
            let label = format!("{name}: export {object_name} data");
            let serial_size = er.i32_size(&label)?;
            let serial_offset = er.i32_size(&label)?;
            er.skip(4)?; // exportFlags
            let net_objects = er.i32_size(&format!("{name}: export net objects"))?;
            if net_objects > er.remaining() / 4 {
                return Err(GameDataError::Truncated(format!(
                    "{name}: export net objects"
                )));
            }
            er.skip(net_objects * 4)?;
            er.skip(16 + 4)?; // GUID, package flags
            if serial_offset > bytes.len() || serial_size > bytes.len() - serial_offset {
                return Err(GameDataError::Truncated(label));
            }
            exports.push(ExportEntry {
                class_ref,
                super_ref,
                outer,
                object_name,
                archetype,
                object_flags,
                serial_size,
                serial_offset,
            });
        }

        // Indexes. An export whose path cannot be built (a broken outer chain) is left out of
        // the path index; when two exports share a path the first wins.
        let mut path_index = HashMap::with_capacity(exports.len());
        let mut class_names = Vec::with_capacity(exports.len());
        let mut class_index: HashMap<String, Vec<usize>> = HashMap::new();
        for i in 0..exports.len() {
            if let Some(path) = export_ref(i).and_then(|r| path_of(&imports, &exports, r).ok()) {
                path_index.entry(path).or_insert(i);
            }
            let class = class_name(&imports, &exports, i).to_string();
            class_index.entry(class.clone()).or_default().push(i);
            class_names.push(class);
        }

        Ok(Self {
            name: name.to_string(),
            version,
            licensee,
            names,
            imports,
            exports,
            bytes,
            path_index,
            class_index,
            class_names,
        })
    }

    /// The uncompressed package bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The full dotted path of an object reference, built by walking its outers. Null gives "".
    pub fn path_of(&self, reference: i32) -> Result<String> {
        path_of(&self.imports, &self.exports, reference)
    }

    /// The full dotted path of export `index`.
    pub fn export_path(&self, index: usize) -> Result<String> {
        let reference = export_ref(index)
            .ok_or_else(|| GameDataError::BadReference(format!("export index {index}")))?;
        self.path_of(reference)
    }

    /// The class name of export `index`: "Class" for a null class reference, "" when the
    /// index or its class reference is out of range.
    pub fn class_name_of_export(&self, index: usize) -> &str {
        self.class_names.get(index).map_or("", String::as_str)
    }

    /// The export with this full path, if any.
    pub fn export_index(&self, path: &str) -> Option<usize> {
        self.path_index.get(path).copied()
    }

    /// The indices of every export of this class, in table order.
    pub fn exports_of_class(&self, class: &str) -> &[usize] {
        self.class_index.get(class).map_or(&[], Vec::as_slice)
    }

    /// Every indexed export path with its export index.
    pub(crate) fn export_paths(&self) -> impl Iterator<Item = (&str, usize)> {
        self.path_index
            .iter()
            .map(|(path, &index)| (path.as_str(), index))
    }

    /// Reads an FName against this package's name table.
    pub(crate) fn read_name(&self, r: &mut ByteReader<'_>) -> Result<String> {
        read_name(r, &self.names, &self.name)
    }
}

/// The reference value for export `index`, when it fits.
fn export_ref(index: usize) -> Option<i32> {
    i32::try_from(index).ok()?.checked_add(1)
}

/// An FName is an i32 name index plus an i32 number; a number above 0 appends `_<number-1>`.
fn read_name(r: &mut ByteReader<'_>, names: &[String], package: &str) -> Result<String> {
    let (index, number) = (r.i32()?, r.i32()?);
    let base = usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .ok_or_else(|| GameDataError::BadReference(format!("{package}: name index {index}")))?;
    Ok(if number > 0 {
        format!("{base}_{}", number - 1)
    } else {
        base.clone()
    })
}

fn path_of(imports: &[ImportEntry], exports: &[ExportEntry], start: i32) -> Result<String> {
    let mut parts: Vec<&str> = Vec::new();
    let mut reference = start;
    while reference != 0 {
        if parts.len() >= MAX_OUTER_DEPTH {
            return Err(GameDataError::BadReference(format!(
                "outer chain deeper than {MAX_OUTER_DEPTH}"
            )));
        }
        // `unsigned_abs` cannot overflow, unlike negating i32::MIN.
        let index = reference.unsigned_abs() as usize - 1;
        if reference > 0 {
            let e = exports
                .get(index)
                .ok_or_else(|| GameDataError::BadReference(format!("export {reference}")))?;
            parts.push(&e.object_name);
            reference = e.outer;
        } else {
            let i = imports
                .get(index)
                .ok_or_else(|| GameDataError::BadReference(format!("import {reference}")))?;
            parts.push(&i.object_name);
            reference = i.outer;
        }
    }
    parts.reverse();
    Ok(parts.join("."))
}

fn class_name<'a>(imports: &'a [ImportEntry], exports: &'a [ExportEntry], index: usize) -> &'a str {
    let class = exports[index].class_ref;
    if class == 0 {
        return "Class";
    }
    let k = class.unsigned_abs() as usize - 1;
    if class > 0 {
        exports.get(k).map_or("", |e| e.object_name.as_str())
    } else {
        imports.get(k).map_or("", |i| i.object_name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::decompress;
    use crate::test_support::{chunked, Pkg, EXPORT_SIZE};

    #[test]
    fn parses_synthetic_tables() {
        let mut b = Pkg::new();
        let class = b.class("Class");
        let pkg_ref = b.raw_export(class, 0, "Pkg", vec![]);
        let group = b.raw_export(class, pkg_ref, "Group", vec![]);
        b.raw_export(class, group, "Obj", vec![1, 2, 3]);
        b.number_last_export(4);
        let pkg = UPackage::parse("t", b.build()).unwrap();

        assert_eq!((pkg.version, pkg.licensee), (832, 46));
        assert_eq!((pkg.imports.len(), pkg.exports.len()), (1, 3));
        assert_eq!(pkg.path_of(3).unwrap(), "Pkg.Group.Obj_3");
        assert_eq!(pkg.export_path(2).unwrap(), "Pkg.Group.Obj_3");
        assert_eq!(pkg.export_index("Pkg.Group.Obj_3"), Some(2));
        assert_eq!(pkg.export_index("Pkg.Group.Obj"), None);
        assert_eq!(pkg.path_of(-1).unwrap(), "Class");
        assert_eq!(pkg.path_of(0).unwrap(), "");
        assert_eq!(pkg.class_name_of_export(2), "Class");
        assert_eq!(pkg.class_name_of_export(99), "");
        assert_eq!(pkg.exports_of_class("Class"), [0, 1, 2]);
        assert!(pkg.exports_of_class("Other").is_empty());
        assert_eq!(pkg.exports[2].serial_size, 3);
        let e = &pkg.exports[2];
        assert_eq!(
            pkg.bytes()[e.serial_offset..e.serial_offset + e.serial_size],
            [1, 2, 3]
        );
        for bad in [9, -9, i32::MAX, i32::MIN] {
            assert!(
                matches!(pkg.path_of(bad), Err(GameDataError::BadReference(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn outer_cycle_is_an_error() {
        let mut b = Pkg::new();
        b.raw_export(0, 2, "A", vec![]);
        b.raw_export(0, 1, "B", vec![]);
        let pkg = UPackage::parse("t", b.build()).unwrap();
        assert_eq!(pkg.class_name_of_export(0), "Class");
        assert_eq!(
            pkg.path_of(1),
            Err(GameDataError::BadReference(
                "outer chain deeper than 64".into()
            ))
        );
        // Objects without a path are left out of the path index, not fatal.
        assert_eq!(pkg.export_paths().count(), 0);
    }

    #[test]
    fn first_export_wins_a_shared_path() {
        let mut b = Pkg::new();
        b.raw_export(0, 0, "Same", vec![1]);
        b.raw_export(0, 0, "Same", vec![2]);
        let pkg = UPackage::parse("t", b.build()).unwrap();
        assert_eq!(pkg.export_index("Same"), Some(0));
    }

    #[test]
    fn rejects_other_versions_and_non_packages() {
        let mut bytes = Pkg::new().build();
        bytes[4] = 0x64; // version 868
        assert_eq!(
            UPackage::parse("t", bytes).unwrap_err(),
            GameDataError::UnsupportedPackage {
                version: 868,
                licensee: 46
            }
        );
        let mut bytes = Pkg::new().build();
        bytes[6] = 47;
        assert_eq!(
            UPackage::parse("t", bytes).unwrap_err(),
            GameDataError::UnsupportedPackage {
                version: 832,
                licensee: 47
            }
        );
        let mut bytes = Pkg::new().build();
        bytes[0] = 0;
        assert!(matches!(
            UPackage::parse("t", bytes),
            Err(GameDataError::NotAPackage(_))
        ));
        assert!(matches!(
            UPackage::parse("t", vec![1, 2, 3]),
            Err(GameDataError::Truncated(_))
        ));
    }

    #[test]
    fn rejects_out_of_range_tables() {
        let mut b = Pkg::new();
        b.raw_export(0, 0, "A", vec![9]);
        let good = b.build();
        let patched = |offset: usize, value: i32| {
            let mut bytes = good.clone();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            UPackage::parse("t", bytes)
        };
        // Header words: name count/offset at 20/24, export 28/32, import 36/40.
        for offset in [20, 24, 28, 32, 36, 40] {
            for value in [i32::MAX, -1] {
                assert!(
                    patched(offset, value).is_err(),
                    "header word at {offset} = {value}"
                );
            }
        }
        let export_offset = i32::from_le_bytes(good[32..36].try_into().unwrap()) as usize;
        // A name index outside the name table.
        assert!(matches!(
            patched(export_offset + 12, 99),
            Err(GameDataError::BadReference(_))
        ));
        // Serial size and offset outside the file; a huge net-object count.
        assert!(matches!(
            patched(export_offset + 32, i32::MAX),
            Err(GameDataError::Truncated(_))
        ));
        assert!(matches!(
            patched(export_offset + 36, i32::MAX),
            Err(GameDataError::Truncated(_))
        ));
        assert!(patched(export_offset + 36, -4).is_err());
        assert!(matches!(
            patched(export_offset + 44, i32::MAX),
            Err(GameDataError::Truncated(_))
        ));
        assert_eq!(export_offset + EXPORT_SIZE, good.len() - 1);
        // Every truncation is an error, never a panic.
        for len in 0..good.len() - 1 {
            assert!(UPackage::parse("t", good[..len].to_vec()).is_err(), "{len}");
        }
    }

    #[test]
    fn chunked_package_parses_ignoring_compression_flags() {
        let mut b = Pkg::new();
        b.raw_export(0, 0, "A", vec![9]);
        let decompressed = decompress(&chunked(&b.build(), 0x20000), "c").unwrap();
        let pkg = UPackage::parse("c", decompressed).unwrap();
        assert_eq!(pkg.export_index("A"), Some(0));
    }

    #[test]
    fn opening_a_missing_file_is_install_not_found() {
        let missing = std::env::temp_dir().join("gk-gamedata-no-such-file.upk");
        assert!(matches!(
            UPackage::open(&missing),
            Err(GameDataError::InstallNotFound(_))
        ));
    }
}
