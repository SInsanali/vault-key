//! The file formats on small hand-made packages: every reader gives the right answer on good
//! data, and nothing panics on truncated or corrupted data (results do not matter there; not
//! panicking does).

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gamedata::package::UPackage;
use gamemesh::balances::weapon_balances;
use gamemesh::material::{GunSkin, MaterialParameters, SkinDetail};
use gamemesh::picture::whole_gestalt_scene;
use gamemesh::render::{render, render_with, View};
use gamemesh::texture::{PixelFormat, Texture};
use gamemesh::{GestaltMesh, PartLook, Section, SkeletalMesh};
use support::{mesh_native, Pkg, Rng};

const SQUARE: [[f32; 3]; 4] = [[0.0, -4.0, -2.0], [0.0, 4.0, -2.0], [0.0, 4.0, 2.0], [0.0, -4.0, 2.0]];
const CACHE_OFFSET: usize = 7;

/// An 8×8 DXT1 mip: four blocks, the first pure red.
fn large_mip() -> Vec<u8> {
    let mut data = vec![0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
    data.extend([0x1F, 0x00, 0x1F, 0x00, 0, 0, 0, 0].repeat(3));
    data
}

/// The texture cache file for [`sample`]: junk, then the large mip as one LZO chunk stream.
fn cache_file() -> Vec<u8> {
    let data = large_mip();
    let block = lzo1x::compress_literals(&data);
    let mut out = vec![0xEE; CACHE_OFFSET];
    for v in [0x9E2A_83C1u32, 0x0002_0000, block.len() as u32, data.len() as u32, block.len() as u32, data.len() as u32] {
        out.extend(v.to_le_bytes());
    }
    out.extend(block);
    out
}

/// One of each kind of object the crate reads. Returns the package and the size of the
/// texture cache entry.
fn sample() -> Pkg {
    let mut p = Pkg::new();
    let (root, barrel) = (p.name("Root"), p.name("Barrel"));
    let native = mesh_native(&SQUARE, &[0, 1, 2, 0, 2, 3], &[(root, 0), (barrel, 0)]);
    let body = p.body(&[], &native);
    let mesh = p.export("SkeletalMesh", 0, "Mesh", body);

    let fragment = |p: &mut Pkg, name: &str, first: i32| {
        [p.name_at("SkeletalMeshFragmentName", 0, name), p.int("MaterialIndex", 0), p.int("FirstIndex", first), p.int("NumPrimitives", 1)].concat()
    };
    let parts = [fragment(&mut p, "FragA", 0), fragment(&mut p, "FragB", 3)];
    let info = p.structs("Parts", &parts);
    let tags = [p.object("GestaltSkeletalMesh", mesh), p.structs("GestaltInfos", &[info])];
    let body = p.body(&tags, &[]);
    p.export("GestaltSkeletalMeshDefinition", 0, "Def", body);

    let cache_len = cache_file().len() - CACHE_OFFSET;
    let mut native = vec![0u8; 16];
    native.extend(2i32.to_le_bytes());
    for v in [0x11, 32, cache_len as i32, CACHE_OFFSET as i32, 8, 8] {
        native.extend(v.to_le_bytes());
    }
    for v in [0i32, 8, 8, 0] {
        native.extend(v.to_le_bytes());
    }
    native.extend([0xE0, 0x07, 0xE0, 0x07, 0, 0, 0, 0]); // 4×4, pure green
    native.extend(4i32.to_le_bytes());
    native.extend(4i32.to_le_bytes());
    native.extend([0u8; 36]);
    let tags = [p.enumeration("Format", "EPixelFormat", "PF_DXT1"), p.name_at("TextureFileCacheName", 0, "TestCache")];
    let body = p.body(&tags, &native);
    let texture = p.export("Texture2D", 0, "Tex", body);

    let body = p.body(&[], &[]);
    let master = p.export("Material", 0, "Master", body);
    let scalar = [p.name_at("ParameterName", 0, "p_HighlightsIntensity"), p.float("ParameterValue", 3.0)].concat();
    let colour: Vec<u8> = [0.5f32, 2.0, 0.25, 1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
    let vector = [p.name_at("ParameterName", 0, "p_AColorMidtone"), p.raw_struct("ParameterValue", "LinearColor", &colour)].concat();
    let masks = [p.name_at("ParameterName", 0, "p_Masks"), p.object("ParameterValue", texture)].concat();
    let diffuse = [p.name_at("ParameterName", 0, "P_Diffuse"), p.object("ParameterValue", texture)].concat();
    let pattern = [p.name_at("ParameterName", 0, "p_Pattern"), p.object("ParameterValue", texture)].concat();
    let decal = [p.name_at("ParameterName", 0, "p_Decal"), p.object("ParameterValue", texture)].concat();
    let four = |p: &mut Pkg, name: &str, v: [f32; 4]| {
        let bytes: Vec<u8> = v.iter().flat_map(|c| c.to_le_bytes()).collect();
        [p.name_at("ParameterName", 0, name), p.raw_struct("ParameterValue", "LinearColor", &bytes)].concat()
    };
    let pattern_scale = four(&mut p, "p_PatternScalePosition", [8.0, 8.0, 0.25, 0.0]);
    let pattern_strength = four(&mut p, "p_PatternChannelScale", [1.0, 0.5, 0.0, 0.0]);
    let decal_scale = four(&mut p, "p_DecalScalePosition", [4.0, 4.0, 0.5, 0.5]);
    let replace = [p.name_at("ParameterName", 0, "p_ReplacePattern"), p.float("ParameterValue", 1.0)].concat();
    let tags = [
        p.structs("ScalarParameterValues", &[scalar, replace]),
        p.structs("VectorParameterValues", &[vector, pattern_scale, pattern_strength, decal_scale]),
        p.structs("TextureParameterValues", &[masks, diffuse, pattern, decal]),
        p.object("Parent", master),
    ];
    let mut native = vec![3u8; 11];
    native.extend(p.fname("p_WeapClassSelect"));
    for flag in [0i32, 0, 1, 0, 1] {
        native.extend(flag.to_le_bytes());
    }
    native.extend([9u8; 16]);
    native.extend(p.fname("p_PatternChannel"));
    for flag in [1i32, 1, 1, 0, 1] {
        native.extend(flag.to_le_bytes());
    }
    native.extend([8u8; 16]);
    native.extend(p.fname("sw_FlipDecalOnRightSide"));
    for flag in [1i32, 1] {
        native.extend(flag.to_le_bytes());
    }
    native.extend([7u8; 16]);
    let body = p.body(&tags, &native);
    let skin = p.export("MaterialInstanceConstant", 0, "Skin", body);

    let tags = [
        p.boolean("bIsGestaltMode", true),
        p.name_at("GestaltModeSkeletalMeshName", 0, "FragA"),
        p.name_at("AdditionalGestaltModeSkeletalMeshNames", 1, "FragB"),
        p.object("Material", skin),
    ];
    let body = p.body(&tags, &[]);
    let part = p.export("WeaponPartDefinition", 0, "Part", body);

    let entry = p.object("Part", part);
    let list = [p.boolean("bEnabled", true), p.structs("WeightedParts", &[entry])].concat();
    let tags = [p.tagged_struct("BodyPartData", "WeaponCustomPartTypeData", &list)];
    let body = p.body(&tags, &[]);
    let collection = p.export("WeaponPartListCollectionDefinition", 0, "List", body);
    let tags = [p.object("InventoryDefinition", part), p.object("WeaponPartListCollection", collection)];
    let body = p.body(&tags, &[]);
    p.export("WeaponBalanceDefinition", 0, "Balance", body);
    p
}

/// A folder of its own in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gamemesh-test-{}-{label}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write_cache(&self, bytes: &[u8]) {
        std::fs::write(self.0.join("TestCache.tfc"), bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app draws on a thread of its own, so the entry point must be able to move there.
#[test]
fn pictures_can_move_to_another_thread() {
    fn is_send<T: Send>() {}
    is_send::<gamemesh::Pictures>();
    is_send::<gamemesh::PictureError>();
}

fn index(pkg: &UPackage, path: &str) -> usize {
    pkg.export_index(path).unwrap_or_else(|| panic!("no {path}"))
}

#[test]
fn skeletal_mesh_reads() {
    let pkg = sample().parse().unwrap();
    let m = SkeletalMesh::from_export(&pkg, index(&pkg, "Mesh")).unwrap();
    assert_eq!(m.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), ["Root", "Barrel"]);
    assert_eq!(m.bones[1].parent, 0);
    assert_eq!(m.sections, [Section { material_index: 0, chunk_index: 0, base_index: 0, triangle_count: 2 }]);
    assert_eq!(m.indices, [0, 1, 2, 0, 2, 3]);
    assert_eq!(m.vertices.len(), 4);
    assert_eq!(m.vertices[1].position, [0.0, 4.0, -2.0]);
    assert_eq!(m.vertices[1].uv, [1.0, 0.5]);
    assert_eq!(m.vertices[1].bone, 1);
    assert!((m.vertices[0].normal[0] + 1.0).abs() < 1e-6 && m.vertices[0].normal[1].abs() < 0.01);
    assert_eq!((m.materials.as_slice(), m.uv_set_count, m.extra_uvs.len(), m.lod_count), (&[0][..], 1, 0, 1));
    assert_eq!(m.second_uv(2), [0.0, 0.0]);
    assert_eq!(m.bounding_box(&m.indices), Some(([0.0, -4.0, -2.0], [0.0, 4.0, 2.0])));
    assert_eq!(m.bounding_box(&[]), None);
    // Another class is refused, not misread.
    assert!(SkeletalMesh::from_export(&pkg, index(&pkg, "Tex")).is_err());
    assert!(SkeletalMesh::from_export(&pkg, 999).is_err());
}

#[test]
fn gestalt_definition_reads() {
    let pkg = sample().parse().unwrap();
    let g = GestaltMesh::from_export(&pkg, index(&pkg, "Def")).unwrap();
    assert_eq!(g.definition_path, "Def");
    let runs: Vec<(&str, u32, u32)> = g.fragments.iter().map(|f| (f.name.as_str(), f.first_index, f.triangle_count)).collect();
    assert_eq!(runs, [("FragA", 0, 1), ("FragB", 3, 1)]);
    assert_eq!(g.indices_of(g.fragments_named("fragb").next().unwrap()), [0, 2, 3]);
    assert_eq!(g.fragments_named("FragC").count(), 0);
    // Both triangles face the default camera and fill the middle of the picture.
    let image = render(&whole_gestalt_scene(&g), &View { width: 40, height: 20, line_width: 0.0, ..View::default() });
    assert_eq!(image.rgba[(10 * 40 + 20) * 4 + 3], 255);
    assert_eq!(image.rgba[3], 0);
    assert!(GestaltMesh::from_export(&pkg, index(&pkg, "Mesh")).is_err());
}

#[test]
fn texture_reads_inline_and_from_the_cache() {
    let dir = TempDir::new("texture");
    dir.write_cache(&cache_file());
    let pkg = sample().parse().unwrap();
    let t = Texture::from_export(&pkg, index(&pkg, "Tex")).unwrap();
    assert_eq!((t.format, t.cache_name.as_deref()), (PixelFormat::Dxt1, Some("TestCache")));
    assert_eq!(t.mips.iter().map(|m| (m.width, m.is_inline())).collect::<Vec<_>>(), [(8, false), (4, true)]);
    let small = t.decode(t.mip_for(4).unwrap(), &dir.0).unwrap();
    assert_eq!((small.width, small.height, &small.rgba[..4]), (4, 4, &[0, 255, 0, 255][..]));
    let large = t.decode(t.mip_for(4096).unwrap(), &dir.0).unwrap();
    assert_eq!((large.width, large.byte_len()), (8, 8 * 8 * 4));
    assert_eq!((&large.rgba[..4], &large.rgba[4 * 4..4 * 4 + 4]), (&[255, 0, 0, 255][..], &[0, 0, 255, 255][..]));
    // No cache file where it should be: an error for the large mip only.
    let empty = TempDir::new("texture-empty");
    assert!(t.decode(&t.mips[0], &empty.0).is_err());
    assert!(t.decode(&t.mips[1], &empty.0).is_ok());
}

#[test]
fn material_part_and_balance_read() {
    let pkg = sample().parse().unwrap();
    let m = MaterialParameters::read(&pkg, "Skin").unwrap();
    assert_eq!(m.chain, ["Skin", "Master"]);
    assert_eq!(m.scalar("P_HIGHLIGHTSINTENSITY"), Some(3.0));
    assert_eq!(m.vector("p_AColorMidtone"), Some([0.5, 2.0, 0.25, 1.0]));
    assert_eq!((m.texture("p_Masks"), m.texture("p_diffuse")), (Some("Tex"), Some("Tex")));
    assert_eq!(m.channel("p_WeapClassSelect"), Some(2));
    assert_eq!(GunSkin::texture_paths(&m), Some(("Tex", "Tex")));
    assert_eq!((GunSkin::pattern_path(&m), GunSkin::decal_path(&m)), (Some("Tex"), Some("Tex")));
    assert_eq!(m.channel_mask("p_PatternChannel"), Some([true, true, true, false]));
    assert_eq!(m.channel_mask("sw_FlipDecalOnRightSide").map(|f| f[0]), Some(true));
    assert_eq!(m.vector("p_PatternScalePosition"), Some([8.0, 8.0, 0.25, 0.0]));
    assert_eq!(m.scalar("p_ReplacePattern"), Some(1.0));
    assert!(MaterialParameters::read(&pkg, "Nowhere").is_err());

    let look = PartLook::read(&pkg, &pkg.properties_of_export(index(&pkg, "Part")).unwrap());
    assert_eq!(look, PartLook { fragments: vec!["FragA".into(), "FragB".into()], material: Some("Skin".into()) });
    // A part that is not in gestalt mode shows nothing, whatever it names.
    assert_eq!(PartLook::read(&pkg, &pkg.properties_of_export(index(&pkg, "Skin")).unwrap()), PartLook::default());

    let balances = weapon_balances(&pkg);
    assert_eq!(balances.len(), 1);
    assert_eq!((balances[0].balance.as_str(), balances[0].weapon_type.as_deref()), ("Balance", Some("Part")));
    assert_eq!(balances[0].slots[0], ["Part"]);
    assert!(balances[0].slots[1..].iter().all(Vec::is_empty));
}

/// Every reader on every export, then the renderer and the skin on whatever came back.
fn exercise(pkg: &UPackage, cache_dir: &Path) {
    let mut pixels = None;
    for i in 0..pkg.exports.len() + 1 {
        let _ = SkeletalMesh::from_export(pkg, i);
        if let Ok(gestalt) = GestaltMesh::from_export(pkg, i) {
            for fragment in &gestalt.fragments {
                let _ = gestalt.indices_of(fragment);
                let _ = gestalt.mesh.bounding_box(gestalt.indices_of(fragment));
            }
            let _ = render(&whole_gestalt_scene(&gestalt), &View { width: 12, height: 6, supersample: 1, ..View::default() });
        }
        if let Ok(texture) = Texture::from_export(pkg, i) {
            let _ = texture.mip_for(1);
            for mip in &texture.mips {
                if let Ok(p) = texture.decode(mip, cache_dir) {
                    let _ = (p.sample(0.3, 7.5), p.sample_linear(-2.0, f32::NAN));
                    pixels = Some(Arc::new(p));
                }
            }
        }
        if let Ok(props) = pkg.properties_of_export(i) {
            let _ = PartLook::read(pkg, &props);
        }
        if let Ok(path) = pkg.export_path(i) {
            if let (Ok(params), Some(px)) = (MaterialParameters::read(pkg, &path), &pixels) {
                let _ = (params.channel("p_WeapClassSelect"), params.vector("p_AColorMidtone"));
                let skin = GunSkin::new(&params, px.clone(), px.clone(), SkinDetail::Textured).with_pattern(&params, px.clone()).with_decal(&params, px.clone());
                if let Ok(gestalt) = GestaltMesh::from_export(pkg, 1) {
                    let _ = render_with(&whole_gestalt_scene(&gestalt), &View { width: 8, height: 4, ..View::default() }, &skin);
                }
            }
        }
    }
    let _ = weapon_balances(pkg);
}

#[test]
fn truncated_and_corrupted_objects_never_panic() {
    let dir = TempDir::new("robust");
    dir.write_cache(&cache_file());
    let good = sample();
    exercise(&good.parse().unwrap(), &dir.0);
    let mut rng = Rng(0x00C0_FFEE);
    let mut parsed = 0;
    for target in 0..good.exports.len() {
        let body = good.exports[target].3.clone();
        let mut attempt = |bytes: Vec<u8>| {
            let mut p = good.clone();
            p.exports[target].3 = bytes;
            if let Ok(pkg) = p.parse() {
                parsed += 1;
                exercise(&pkg, &dir.0);
            }
        };
        // Every truncation.
        for len in 0..body.len() {
            attempt(body[..len].to_vec());
        }
        // Corruption: one to four bytes changed, to anything or to an extreme value.
        for _ in 0..400 {
            let mut bytes = body.clone();
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(bytes.len());
                bytes[at] = match rng.below(4) {
                    0 => 0xFF,
                    1 => 0x00,
                    2 => 0x7F,
                    _ => rng.next() as u8,
                };
            }
            attempt(bytes);
        }
        // A four-byte field set to a huge or negative number, at every offset.
        for at in 0..body.len().saturating_sub(3) {
            for value in [i32::MAX, -1, 0x4000_0000, 65_536] {
                let mut bytes = body.clone();
                bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
                attempt(bytes);
            }
        }
        // Noise of the same length.
        attempt((0..body.len()).map(|_| rng.next() as u8).collect());
    }
    assert!(parsed > 5_000, "only {parsed} damaged packages still parsed");
}

#[test]
fn damaged_mesh_data_never_panics() {
    let names: Vec<String> = ["None", "Root", "Barrel"].map(String::from).to_vec();
    let good = mesh_native(&SQUARE, &[0, 1, 2, 0, 2, 3], &[(1, 0), (2, 0)]);
    assert!(SkeletalMesh::parse(&good, &names).is_ok());
    // Every truncation is an error (the 20 trailing bytes are not needed).
    for len in 0..good.len() - 20 {
        assert!(SkeletalMesh::parse(&good[..len], &names).is_err(), "length {len}");
    }
    // An index past the vertex count, a bone name outside the name table, a bad parent.
    assert!(SkeletalMesh::parse(&mesh_native(&SQUARE, &[0, 1, 4], &[(1, 0)]), &names).is_err());
    assert!(SkeletalMesh::parse(&mesh_native(&SQUARE, &[0, 1, 2], &[(9, 0)]), &names).is_err());
    assert!(SkeletalMesh::parse(&mesh_native(&SQUARE, &[0, 1, 2], &[(1, 4)]), &names).is_err());
    let mut rng = Rng(77);
    for _ in 0..20_000 {
        let mut bytes = good.clone();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] = rng.next() as u8;
        }
        let _ = SkeletalMesh::parse(&bytes, &names);
    }
    for at in 0..good.len() - 3 {
        for value in [i32::MAX, -1, i32::MIN, 0x0100_0000] {
            let mut bytes = good.clone();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            let _ = SkeletalMesh::parse(&bytes, &names);
        }
    }
    let noise: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();
    assert!(SkeletalMesh::parse(&noise, &names).is_err());
    assert!(SkeletalMesh::parse(&[], &names).is_err());
}

#[test]
fn damaged_texture_cache_never_panics() {
    let dir = TempDir::new("cache");
    let pkg = sample().parse().unwrap();
    let texture = Texture::from_export(&pkg, index(&pkg, "Tex")).unwrap();
    let good = cache_file();
    let mut rng = Rng(5);
    let attempt = |bytes: &[u8]| -> bool {
        dir.write_cache(bytes);
        texture.decode(&texture.mips[0], &dir.0).is_err()
    };
    // Every truncated cache file is an error.
    for len in 0..good.len() {
        assert!(attempt(&good[..len]), "length {len}");
    }
    for at in CACHE_OFFSET..good.len() - 3 {
        for value in [u32::MAX, 0, 1, 0x7FFF_FFFF] {
            let mut bytes = good.clone();
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            attempt(&bytes);
        }
    }
    for _ in 0..300 {
        let mut bytes = good.clone();
        let at = rng.below(bytes.len());
        bytes[at] = rng.next() as u8;
        attempt(&bytes);
    }
    // A cache name that tries to leave the folder is refused.
    let mut sneaky = texture.clone();
    sneaky.cache_name = Some("..\\TestCache".into());
    assert!(sneaky.decode(&sneaky.mips[0], &dir.0).is_err());
}
