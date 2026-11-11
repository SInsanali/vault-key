//! Writes the program's Windows resources (its icon, version details and manifest) as a
//! compiled `.res` file, which the linker takes as it is. Done by hand so that the build
//! needs no resource compiler: the toolchain this project builds with has none.

use std::path::Path;

/// What the version details say. Explorer shows these under Properties → Details.
pub struct VersionInfo<'a> {
    /// The version as text, from the workspace version (it may carry a pre-release part).
    pub version: &'a str,
    /// Its numeric part: major, minor, patch.
    pub numeric: [u16; 3],
    pub product_name: &'a str,
    pub file_description: &'a str,
    pub company_name: &'a str,
    pub copyright: &'a str,
    pub original_filename: &'a str,
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;
const LANG_EN_US: u16 = 0x0409;
const CODEPAGE_UNICODE: u16 = 0x04B0;

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// One entry of a `.res` file: a header naming the resource by numeric type and id, then its data.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, language: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes()); // header size
    for number in [kind, id] {
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
        out.extend_from_slice(&number.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&0x1030u16.to_le_bytes()); // moveable, pure, discardable
    out.extend_from_slice(&language.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// Splits an `.ico` file into its images and the group entry that lists them.
fn icon_resources(out: &mut Vec<u8>, ico: &[u8]) {
    let u16_at = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]) as usize;
    assert!(ico.len() >= 6 && u16_at(0) == 0 && u16_at(2) == 1, "not an .ico file");
    let count = u16_at(4) as usize;
    let mut group = Vec::new();
    group.extend_from_slice(&ico[..6]);
    for index in 0..count {
        let entry = 6 + index * 16;
        let (size, offset) = (u32_at(entry + 8), u32_at(entry + 12));
        let id = index as u16 + 1;
        resource(out, RT_ICON, id, LANG_EN_US, &ico[offset..offset + size]);
        // The group entry is the file's entry with the image's offset replaced by its id.
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(out, RT_GROUP_ICON, 1, LANG_EN_US, &group);
}

/// One node of the version block: a length, a value length, a type, a name, then the value
/// and any children, each starting on a four-byte boundary.
fn version_node(key: &str, value: &[u8], value_length: u16, is_text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut node = vec![0u8, 0];
    node.extend_from_slice(&value_length.to_le_bytes());
    node.extend_from_slice(&u16::from(is_text).to_le_bytes());
    node.extend_from_slice(&utf16z(key));
    pad4(&mut node);
    node.extend_from_slice(value);
    for child in children {
        pad4(&mut node);
        node.extend_from_slice(child);
    }
    let length = (node.len() as u16).to_le_bytes();
    node[..2].copy_from_slice(&length);
    node
}

fn version_resource(info: &VersionInfo) -> Vec<u8> {
    let [major, minor, patch] = info.numeric;
    let version_ms = (u32::from(major) << 16) | u32::from(minor);
    let version_ls = u32::from(patch) << 16;
    let fixed: Vec<u8> = [
        0xFEEF_04BDu32, // signature
        0x0001_0000,    // structure version
        version_ms,
        version_ls,
        version_ms,
        version_ls,
        0x3F,        // flags mask
        0,           // flags
        0x0004_0004, // made for Windows NT
        1,           // an application
        0,
        0,
        0,
    ]
    .iter()
    .flat_map(|field| field.to_le_bytes())
    .collect();

    let text = |key: &str, value: &str| {
        let value = utf16z(value);
        version_node(key, &value, (value.len() / 2) as u16, true, &[])
    };
    let strings = [
        text("CompanyName", info.company_name),
        text("FileDescription", info.file_description),
        text("FileVersion", info.version),
        text("InternalName", info.product_name),
        text("LegalCopyright", info.copyright),
        text("OriginalFilename", info.original_filename),
        text("ProductName", info.product_name),
        text("ProductVersion", info.version),
    ];
    let table = version_node(&format!("{LANG_EN_US:04x}{CODEPAGE_UNICODE:04x}"), &[], 0, true, &strings);
    let string_file_info = version_node("StringFileInfo", &[], 0, true, &[table]);
    let translation: Vec<u8> = [LANG_EN_US, CODEPAGE_UNICODE].iter().flat_map(|word| word.to_le_bytes()).collect();
    let var_file_info = version_node("VarFileInfo", &[], 0, true, &[version_node("Translation", &translation, 4, false, &[])]);
    version_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[string_file_info, var_file_info])
}

/// Writes the `.res` file.
pub fn write(path: &Path, ico: &[u8], manifest: &str, info: &VersionInfo) {
    let mut out = Vec::new();
    // Every .res file starts with an empty entry.
    resource(&mut out, 0, 0, 0, &[]);
    // The start marker has no flags.
    out[20..22].copy_from_slice(&[0, 0]);
    icon_resources(&mut out, ico);
    resource(&mut out, RT_VERSION, 1, LANG_EN_US, &version_resource(info));
    resource(&mut out, RT_MANIFEST, 1, LANG_EN_US, manifest.as_bytes());
    std::fs::write(path, out).expect("write the resource file");
}
