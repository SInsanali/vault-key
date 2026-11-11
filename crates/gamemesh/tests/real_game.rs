//! Checks against the real game install. Each test is skipped (with a note) when the game is
//! not installed. The install is only read; nothing is written anywhere.
//!
//! Run with `--nocapture` to see the figures quoted in
//! `docs/research/2026-11-09-weapon-pictures-spike.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use gamedata::package::UPackage;
use gamedata::{GameInstall, ItemRefs};
use gamemesh::balances::weapon_balances;
use gamemesh::material::MaterialParameters;
use gamemesh::texture::Texture;
use gamemesh::{fragment_names, Colouring, Facing, GestaltMesh, PictureError, Pictures, SkeletalMesh, Style};
use savekit::SaveDocument;

const INFINITY: &str = "BL2(hwAAAADX0ACE5wRACiGT6cOQhYEQowHG/38JGBkw/v8rxACCgRED)";
const RELIC: &str = "BL2(BwAAAABrxwAYC17YIBGBAABAAMD/////////Pw2AAwA=)";

fn install() -> Option<GameInstall> {
    let found = GameInstall::locate();
    if found.is_none() {
        eprintln!("skipped: no game install found");
    }
    found
}

/// `Startup.upk`, opened once for the tests that look at the package itself.
fn package() -> Option<&'static UPackage> {
    static PACKAGE: OnceLock<Option<UPackage>> = OnceLock::new();
    PACKAGE.get_or_init(|| Some(UPackage::open(&install()?.startup_package()).expect("Startup.upk opens"))).as_ref()
}

/// One shared `Pictures` for the tests that draw.
fn pictures() -> Option<MutexGuard<'static, Pictures>> {
    static PICTURES: OnceLock<Option<Mutex<Pictures>>> = OnceLock::new();
    let shared = PICTURES.get_or_init(|| Some(Mutex::new(Pictures::open(&install()?).expect("pictures open"))));
    let mut guard = shared.as_ref()?.lock().unwrap_or_else(|e| e.into_inner());
    guard.set_style(Style::default());
    Some(guard)
}

fn infinity() -> ItemRefs {
    Pictures::decode(INFINITY).unwrap()
}

/// The weapons in the fixture save, with their names.
fn fixture_weapons(pictures: &Pictures) -> Vec<(String, ItemRefs)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/local/win-throwaway-Save0001.sav");
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("skipped: no fixture save");
        return Vec::new();
    };
    let doc = SaveDocument::from_file_bytes(&bytes).unwrap();
    let refs = |f: &savekit::ItemFields| ItemRefs {
        is_weapon: true,
        set: f.set,
        item_type: f.item_type,
        balance: f.balance,
        manufacturer: f.manufacturer,
        parts: f.parts,
    };
    doc.inventory()
        .iter()
        .filter(|e| e.is_weapon)
        .filter_map(|e| e.fields.as_ref())
        .map(|f| (pictures.source().database().info(&refs(f)).name.unwrap_or_default(), refs(f)))
        .collect()
}

fn base_fragments(pkg: &UPackage) -> BTreeSet<String> {
    let mut known = BTreeSet::new();
    for &index in pkg.exports_of_class("GestaltSkeletalMeshDefinition") {
        for fragment in GestaltMesh::from_export(pkg, index).unwrap().fragments {
            known.insert(fragment.name.to_ascii_lowercase());
        }
    }
    known
}

fn opaque_share(rgba: &[u8]) -> f32 {
    let pixels = rgba.as_chunks::<4>().0;
    pixels.iter().filter(|p| p[3] > 0).count() as f32 / pixels.len() as f32
}

#[test]
fn every_skeletal_mesh_in_startup_parses() {
    let Some(pkg) = package() else { return };
    let meshes = pkg.exports_of_class("SkeletalMesh");
    assert_eq!(meshes.len(), 17);
    for &index in meshes {
        let path = pkg.export_path(index).unwrap();
        let mesh = SkeletalMesh::from_export(pkg, index).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(mesh.lod_count, 1, "{path}");
        assert!(!mesh.vertices.is_empty() && mesh.indices.len().is_multiple_of(3), "{path}");
        let triangles: u32 = mesh.sections.iter().map(|s| s.triangle_count).sum();
        assert_eq!(triangles as usize * 3, mesh.indices.len(), "{path}: sections cover the index buffer");
        // Nearly every vertex is used (a few meshes have a handful that no triangle uses), and
        // all lie inside the stored bounds (with a little slack).
        let used: BTreeSet<u32> = mesh.indices.iter().copied().collect();
        let unused = mesh.vertices.len() - used.len();
        assert!(unused * 100 <= mesh.vertices.len(), "{path}: {unused} unused vertices");
        let (lo, hi) = mesh.bounding_box(&mesh.indices).unwrap();
        for a in 0..3 {
            let slack = mesh.bounds_extent[a] * 0.02 + 0.5;
            assert!(lo[a] >= mesh.bounds_origin[a] - mesh.bounds_extent[a] - slack, "{path}: axis {a} low {lo:?}");
            assert!(hi[a] <= mesh.bounds_origin[a] + mesh.bounds_extent[a] + slack, "{path}: axis {a} high {hi:?}");
        }
        // Normals are unit length (they are stored as bytes, so allow rounding), but for a
        // handful of vertices per mesh that the game stores with a zero normal.
        let bad = mesh
            .vertices
            .iter()
            .filter(|v| (v.normal.iter().map(|c| c * c).sum::<f32>().sqrt() - 1.0).abs() > 0.05)
            .count();
        assert!(bad <= 7, "{path}: {bad} vertices without a unit normal");
        println!(
            "{path}: {} bones, {} vertices ({unused} unused, {bad} without a normal), {} triangles, {} uv sets, box {lo:.1?} .. {hi:.1?}",
            mesh.bones.len(),
            mesh.vertices.len(),
            mesh.indices.len() / 3,
            mesh.uv_set_count
        );
    }
}

#[test]
fn gestalt_definitions_tile_their_meshes() {
    let Some(pkg) = package() else { return };
    let definitions = pkg.exports_of_class("GestaltSkeletalMeshDefinition");
    assert_eq!(definitions.len(), 9);
    for &index in definitions {
        let gestalt = GestaltMesh::from_export(pkg, index).unwrap();
        // The fragments are disjoint runs that together cover the whole index buffer.
        let mut runs: Vec<(u32, u32)> = gestalt.fragments.iter().map(|f| (f.first_index, 3 * f.triangle_count)).collect();
        runs.sort_unstable();
        let mut next = 0;
        for (first, len) in &runs {
            assert_eq!(*first, next, "{}: gap or overlap", gestalt.definition_path);
            next = first + len;
        }
        assert_eq!(next as usize, gestalt.mesh.indices.len(), "{}", gestalt.definition_path);
        // A name usually belongs to one run, but not always: a few names cover several runs.
        let mut repeated = BTreeMap::new();
        for f in &gestalt.fragments {
            *repeated.entry(f.name.to_ascii_lowercase()).or_insert(0usize) += 1;
        }
        repeated.retain(|_, n| *n > 1);
        for (name, n) in &repeated {
            assert_eq!(gestalt.fragments_named(name).count(), *n);
            println!("  {}: {n} runs are named {name}", gestalt.definition_path);
        }
        assert!(repeated.len() <= 2, "{}: {repeated:?}", gestalt.definition_path);
        println!("{}: {} fragments, {} triangles", gestalt.definition_path, gestalt.fragments.len(), next / 3);
    }
}

#[test]
fn every_gestalt_part_names_fragments_that_exist() {
    let Some(pkg) = package() else { return };
    let known = base_fragments(pkg);
    let (mut with_mesh, mut without) = (0, 0);
    for &index in pkg.exports_of_class("WeaponPartDefinition") {
        let names = fragment_names(&pkg.properties_of_export(index).unwrap());
        if names.is_empty() {
            without += 1;
        } else {
            with_mesh += 1;
        }
        for name in names {
            assert!(known.contains(&name.to_ascii_lowercase()), "{}: {name}", pkg.export_path(index).unwrap());
        }
    }
    println!("weapon parts: {with_mesh} show mesh fragments, {without} show none");
    assert_eq!((with_mesh, without), (428, 230));
}

/// DLC packs bring new part definitions but no new gestalt meshes: every DLC weapon part that
/// shows a mesh names fragments of the base game's meshes.
#[test]
fn dlc_weapon_parts_reuse_the_base_meshes() {
    let (Some(pkg), Some(install)) = (package(), install()) else { return };
    let known = base_fragments(pkg);
    let (mut parts, mut with_mesh, mut unknown) = (0, 0, Vec::new());
    for file in install.dlc_startup_packages() {
        let dlc = UPackage::open(&file).unwrap();
        assert!(dlc.exports_of_class("GestaltSkeletalMeshDefinition").is_empty(), "{}", dlc.name);
        for &index in dlc.exports_of_class("WeaponPartDefinition") {
            parts += 1;
            let names = fragment_names(&dlc.properties_of_export(index).unwrap());
            with_mesh += usize::from(!names.is_empty());
            for name in names {
                if !known.contains(&name.to_ascii_lowercase()) {
                    unknown.push(format!("{}: {} -> {name}", dlc.name, dlc.export_path(index).unwrap()));
                }
            }
        }
    }
    println!("DLC startup packages: {parts} weapon parts, {with_mesh} show mesh fragments, {} name an unknown fragment", unknown.len());
    assert!(parts > 200);
    assert!(unknown.is_empty(), "{unknown:?}");
}

/// Which way is which. The decal projection (texture-coordinate set 1) runs left to right and
/// top to bottom on the −X side when the muzzle points left, so lettering reads correctly
/// there without a flip; and shells leave on the +X side. So −X is the weapon's left side and
/// the default view is not a mirror image.
#[test]
fn the_default_view_is_the_unmirrored_left_side() {
    let Some(pkg) = package() else { return };
    for &index in pkg.exports_of_class("GestaltSkeletalMeshDefinition") {
        let gestalt = GestaltMesh::from_export(pkg, index).unwrap();
        if !gestalt.definition_path.starts_with("Weap_") || gestalt.mesh.uv_set_count < 2 || gestalt.definition_path.contains("Grenade") {
            continue;
        }
        // Over the triangles that face −X: does u grow with y (towards the stock, screen
        // right in the default view), and v fall with z (screen down)?
        let (mut u_with_y, mut v_against_z, mut faces, mut uprights) = (0usize, 0usize, 0usize, 0usize);
        for tri in gestalt.mesh.indices.as_chunks::<3>().0 {
            let v = tri.map(|i| gestalt.mesh.vertices[i as usize]);
            if v.iter().any(|v| v.normal[0] > -0.7) {
                continue;
            }
            let uv = tri.map(|i| gestalt.mesh.second_uv(i as usize));
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (dy, dz) = (v[b].position[1] - v[a].position[1], v[b].position[2] - v[a].position[2]);
                let (du, dv) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]);
                if dy.abs() > 1.0 && dz.abs() < 0.2 && du.abs() < 4.0 {
                    faces += 1;
                    u_with_y += usize::from(du * dy > 0.0);
                }
                if dz.abs() > 1.0 && dy.abs() < 0.2 && dv.abs() < 4.0 {
                    uprights += 1;
                    v_against_z += usize::from(dv * dz < 0.0);
                }
            }
        }
        println!(
            "{}: on the -X side u grows towards +Y on {u_with_y} of {faces} level edges; v falls with height on {v_against_z} of {uprights} upright edges",
            gestalt.definition_path
        );
        assert!(faces > 500 && u_with_y * 10 > faces * 9, "{}", gestalt.definition_path);
        assert!(uprights > 300 && v_against_z * 10 > uprights * 9, "{}", gestalt.definition_path);
    }
    // Ejection ports: the sockets named …_EjectPort sit at X ≥ 0 on every rifle body.
    let mut sides = Vec::new();
    for &index in pkg.exports_of_class("SkeletalMeshSocket") {
        let props = pkg.properties_of_export(index).unwrap();
        let name = props.name("SocketName").unwrap_or("");
        if let (true, Some(gamedata::package::PropertyValue::StructRaw { bytes, .. })) = (name.starts_with("AR_") && name.ends_with("_EjectPort"), props.get("RelativeLocation")) {
            sides.push(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
    }
    println!("assault rifle ejection ports at X = {sides:?}");
    assert!(sides.len() >= 5 && sides.iter().all(|&x| x > 2.0));
}

#[test]
fn the_source_opens_without_problems() {
    let Some(pictures) = pictures() else { return };
    let source = pictures.source();
    assert!(source.problems().is_empty(), "{:?}", source.problems());
    assert_eq!(source.weapon_types().count(), 33);
    // 75 shields, grenade mods and relics, built from parts, and 44 class mods, one mesh each.
    assert_eq!(source.item_types().count(), 119);
    let mut gestalts: Vec<&str> = source.gestalt_paths().collect();
    gestalts.sort_unstable();
    let whole: Vec<&&str> = gestalts.iter().filter(|p| p.contains("ClassMod")).collect();
    assert_eq!(gestalts.len() - whole.len(), 9, "six weapon kinds and three item kinds");
    // One class mod mesh per base-game class (Mechromancer and Psycho class mods are DLC).
    assert_eq!(whole.len(), 4, "{whole:?}");
    for path in gestalts {
        assert!(source.gestalt(path).is_ok(), "{path}");
    }
    assert!(source.gestalt("Nowhere.Def").is_err());
}

#[test]
fn known_weapons_assemble_from_their_own_parts() {
    let Some(pictures) = pictures() else { return };
    let source = pictures.source();
    let fragments = |item: &ItemRefs| -> (String, Vec<String>, Option<String>) {
        let a = source.assemble(item).unwrap();
        assert!(a.unresolved_slots.is_empty());
        (a.gestalt.clone(), a.fragments().map(|(_, f)| f.to_string()).collect(), a.material)
    };
    let (gestalt, parts, material) = fragments(&infinity());
    assert_eq!(gestalt, "Weap_Pistol.GestaltDef_Pistol");
    assert_eq!(
        parts,
        ["Pistol_Body_Vladof", "Pistol_Body_Vladof_Var1", "Pistol_Body_Vladof_Var2", "Pistol_Grip_Hyperion", "Pistol_Barrel_Vladof", "Pistol_Scope_Dahl", "Acc_Barrel_Laser"]
    );
    assert_eq!(material.as_deref(), Some("Common_GunMaterials.Materials.Pistol.Mati_VladofLegendaryPistol_Infinity"));

    let weapons = fixture_weapons(&pictures);
    if weapons.is_empty() {
        return;
    }
    let by_name = |name: &str| fragments(&weapons.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no {name}")).1);
    assert_eq!(
        by_name("Gearbox Rifle"),
        (
            "Weap_AssaultRifles.GestaltDef_AssaultRifle".to_string(),
            ["AR_Body_Vladof", "AR_Grip_Jakobs", "AR_Barrel_Bandit", "AR_Stock_Torgue"].map(String::from).to_vec(),
            Some("Common_GunMaterials.Materials.AssaultRifle.Mati_GearboxAR".to_string())
        )
    );
    assert_eq!(by_name("Gearbox Projectile Convergence").1, ["SMG_Body_Hyperion", "SMG_Grip_Bandit", "SMG_Barrel_Tediore", "SMG_Stock_Bandit"]);
    assert_eq!(by_name("Gearbox Callipeen").1, ["SR_Body_Jakobs", "SR_Grip_Hyperion", "SR_Barrel_Dahl", "SR_Scope_Jakobs", "SR_Stock_Vladof"]);
}

#[test]
fn a_code_becomes_a_png() {
    let Some(mut pictures) = pictures() else { return };
    for (width, height) in [(240u32, 120u32), (560, 280)] {
        let png = pictures.weapon_png(INFINITY, width, height).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!((&png[16..20], &png[20..24]), (&width.to_be_bytes()[..], &height.to_be_bytes()[..]));
        assert!(png.len() < (width * height) as usize, "{} bytes for {width}x{height}", png.len());
        assert_eq!(pictures.skin_problem(), None);
        println!("Infinity {width}x{height}: {} bytes", png.len());
    }
    // Leading and trailing blanks in the code do not matter.
    assert!(pictures.weapon_png(&format!("  {INFINITY}\n"), 64, 32).is_ok());

    let image = pictures.weapon_image(&infinity(), 400, 200).unwrap();
    assert_eq!((image.width, image.height, image.rgba.len()), (400, 200, 400 * 200 * 4));
    // Transparent outside the gun (the corners), a gun-sized shape inside.
    for corner in [0, 399, 199 * 400, 199 * 400 + 399] {
        assert_eq!(image.rgba[corner * 4 + 3], 0);
    }
    let covered = opaque_share(&image.rgba);
    println!("Infinity covers {:.0}% of a 400x200 picture", covered * 100.0);
    assert!((0.15..0.75).contains(&covered));
    // The Infinity's skin is dark navy and pale blue: blue is the strongest channel overall.
    let sum = image.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).fold([0u64; 3], |mut s, p| {
        for c in 0..3 {
            s[c] += u64::from(p[c]);
        }
        s
    });
    assert!(sum[2] > sum[0] && sum[2] > sum[1], "{sum:?}");
    // The light rim: the soft edge of the shape is much lighter with it than without.
    let edge_brightness = |image: &gamemesh::render::Image| -> f32 {
        let edge: Vec<u32> = image.rgba.as_chunks::<4>().0.iter().filter(|p| (1..255).contains(&p[3])).map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])).collect();
        edge.iter().sum::<u32>() as f32 / (3.0 * edge.len().max(1) as f32)
    };
    pictures.set_style(Style { rim: None, ..Style::default() });
    let bare = pictures.weapon_image(&infinity(), 400, 200).unwrap();
    println!("edge brightness with the rim {:.0}, without {:.0}", edge_brightness(&image), edge_brightness(&bare));
    assert!(edge_brightness(&image) > edge_brightness(&bare) + 60.0);
    assert!(opaque_share(&image.rgba) > opaque_share(&bare.rgba));

    // The other facing shows the other side: a different picture of about the same size.
    pictures.set_style(Style { facing: Facing::MuzzleRight, ..Style::default() });
    let right = pictures.weapon_image(&infinity(), 400, 200).unwrap();
    assert_ne!(right.rgba, image.rgba);
    assert!((opaque_share(&right.rgba) - covered).abs() < 0.03);
    // The muzzle end (two thin barrels) is on the left by default and on the right here: the
    // grip, the tallest part, is on the other side.
    let column_height = |image: &gamemesh::render::Image, x: usize| (0..200).filter(|y| image.rgba[(y * 400 + x) * 4 + 3] > 0).count();
    assert!(column_height(&image, 300) > column_height(&image, 100));
    assert!(column_height(&right, 100) > column_height(&right, 300));

    // Plain and flat colouring need less, and still draw the same shape.
    for colouring in [Colouring::Plain, Colouring::Flat] {
        pictures.set_style(Style { colouring, ..Style::default() });
        let other = pictures.weapon_image(&infinity(), 400, 200).unwrap();
        assert!((opaque_share(&other.rgba) - covered).abs() < 0.01);
    }
    // Absurd sizes are brought into range, not refused and not attempted.
    pictures.set_style(Style::default());
    let huge = pictures.weapon_image(&infinity(), 100_000, 50_000).unwrap();
    assert!(u64::from(huge.width) * u64::from(huge.height) <= Pictures::MAX_PIXELS && huge.width > 2 * huge.height - 4);
    assert_eq!(pictures.weapon_image(&infinity(), 0, 0).unwrap().rgba.len(), 4);
}

#[test]
fn what_cannot_be_drawn_says_why() {
    let Some(mut pictures) = pictures() else { return };
    assert_eq!(pictures.weapon_png("hello", 64, 32), Err(PictureError::BadCode));
    assert_eq!(pictures.weapon_png("BL2(AAAA)", 64, 32), Err(PictureError::BadCode));
    // A relic is built from parts like a weapon, so it has a picture.
    assert_eq!(&pictures.weapon_png(RELIC, 64, 32).expect("a relic draws")[..4], b"\x89PNG");
    let with = |change: fn(&mut ItemRefs)| {
        let mut item = infinity();
        change(&mut item);
        item
    };
    assert_eq!(pictures.weapon_image(&with(|i| i.set = Some(9)), 64, 32), Err(PictureError::DlcItem));
    assert_eq!(pictures.weapon_image(&with(|i| i.set = Some(255)), 64, 32), Err(PictureError::NothingToDraw));
    assert_eq!(pictures.weapon_image(&with(|i| i.set = None), 64, 32), Err(PictureError::BadCode));
    // The weapon's type number read as an item type names a kind that is not built from parts.
    let as_item = pictures.weapon_image(&with(|i| i.is_weapon = false), 64, 32).err();
    assert!(matches!(as_item, Some(PictureError::NotAWeapon | PictureError::UnknownParts)), "{as_item:?}");
    // A type or a part that no table lists.
    assert_eq!(pictures.weapon_image(&with(|i| i.item_type = Some(0x1F7F)), 64, 32), Err(PictureError::UnknownParts));
    assert_eq!(pictures.weapon_image(&with(|i| i.parts[2] = Some(0x1F7F0)), 64, 32), Err(PictureError::UnknownParts));
    // No part with a model at all.
    assert_eq!(pictures.weapon_image(&with(|i| i.parts = [Some(131_071); 11]), 64, 32), Err(PictureError::NothingToDraw));
    // An unknown skin does not stop the picture; it is drawn in plain colours and says so.
    let image = pictures.weapon_image(&with(|i| i.parts[8] = Some(131_071)), 64, 32).unwrap();
    assert!(opaque_share(&image.rgba) > 0.1 && pictures.skin_problem().is_some());
    for e in [PictureError::BadCode, PictureError::NotAWeapon, PictureError::NothingToDraw, PictureError::DlcItem, PictureError::UnknownParts, PictureError::GameData("x".into())] {
        assert!(e.message().ends_with('.') && e.to_string().starts_with(e.message()));
    }
}

/// Every balance's part lists, each part of each slot at least once: all assemble, all draw
/// as one connected shape of a sensible size.
#[test]
fn every_balance_assembles_and_draws() {
    let (Some(pkg), Some(mut pictures)) = (package(), pictures()) else { return };
    pictures.set_style(Style { colouring: Colouring::Plain, ..Style::default() });
    let balances = weapon_balances(pkg);
    assert_eq!(balances.len(), 242);
    let (mut combinations, mut drawn) = (0, 0);
    let mut parts_seen = BTreeSet::new();
    for balance in &balances {
        let weapon_type = balance.weapon_type.as_deref().unwrap_or_else(|| panic!("{}: no weapon type", balance.balance));
        let rounds = balance.slots.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for round in 0..rounds {
            let mut parts: [Option<&str>; 9] = [None; 9];
            for (slot, list) in balance.slots.iter().enumerate() {
                if !list.is_empty() {
                    parts[slot] = Some(list[round % list.len()].as_str());
                }
            }
            let what = format!("{} round {round}", balance.balance);
            let assembly = pictures.source().assemble_paths(weapon_type, &parts).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert!(assembly.unresolved_slots.is_empty(), "{what}: {:?}", assembly.unresolved_slots);
            assert!(assembly.material.is_some(), "{what}: no material");
            parts_seen.extend(assembly.parts.iter().map(|p| p.definition.clone()));
            combinations += 1;
            // Drawing every one takes too long in a test build: every fifth.
            if combinations % 5 == 0 {
                let image = pictures.assembly_image(&assembly, 96, 48).unwrap_or_else(|e| panic!("{what}: {e}"));
                let covered = opaque_share(&image.rgba);
                assert!((0.06..0.7).contains(&covered), "{what}: covers {covered}");
                drawn += 1;
            }
        }
    }
    println!("{} balances, {combinations} combinations assembled, {drawn} drawn, {} distinct parts", balances.len(), parts_seen.len());
    assert!(combinations > 1500 && parts_seen.len() > 640);
}

#[test]
fn textures_stay_within_the_budget() {
    let (Some(pkg), Some(mut pictures)) = (package(), pictures()) else { return };
    // One weapon of each class, large (which uses the 2048 px textures), twice round.
    let mut per_class = BTreeMap::new();
    for balance in weapon_balances(pkg) {
        let Some(weapon_type) = balance.weapon_type.clone() else { continue };
        let parts: [Option<&str>; 9] = std::array::from_fn(|slot| balance.slots[slot].first().map(String::as_str));
        if let Ok(assembly) = pictures.source().assemble_paths(&weapon_type, &parts) {
            per_class.entry(assembly.gestalt.clone()).or_insert(assembly);
        }
    }
    assert_eq!(per_class.len(), 6);
    for _ in 0..2 {
        for assembly in per_class.values() {
            let image = pictures.assembly_image(assembly, 600, 300).unwrap();
            assert_eq!(pictures.skin_problem(), None, "{}", assembly.gestalt);
            assert!(opaque_share(&image.rgba) > 0.08);
            assert!(pictures.texture_bytes() <= Pictures::TEXTURE_BUDGET, "{} bytes held", pictures.texture_bytes());
        }
    }
    println!("decoded textures held after six classes: {} MiB of at most {}", pictures.texture_bytes() >> 20, Pictures::TEXTURE_BUDGET >> 20);
    assert!(pictures.texture_bytes() > 16 << 20);
}

#[test]
fn materials_and_textures_read() {
    let (Some(pkg), Some(install)) = (package(), install()) else { return };
    let dir = install.startup_package().parent().unwrap().to_path_buf();
    let params = MaterialParameters::read(pkg, "Common_GunMaterials.Materials.Pistol.Mati_VladofLegendaryPistol_Infinity").unwrap();
    assert_eq!(params.chain.last().map(String::as_str), Some("Common_Materials.Weapons.Master_Gun"));
    assert_eq!(params.texture("p_Masks"), Some("Weap_Pistol.Tex.Weap_Pistols_Comp"));
    assert_eq!(params.texture("p_Diffuse"), Some("Common_GunMaterials.CompTextures.Weap_LauncherShotgunPistol_Comp"));
    assert_eq!(params.channel("p_WeapClassSelect"), Some(2));
    assert_eq!(params.scalar("p_HighlightsIntensity"), Some(3.0));
    let midtone = params.vector("p_BColorMidtone").unwrap();
    assert!((midtone[0] - 1.0843).abs() < 1e-3 && (midtone[2] - 1.6597).abs() < 1e-3, "{midtone:?}");
    // A skin that inherits through a maker's master instance.
    let inherited = MaterialParameters::read(pkg, "Common_GunMaterials.Materials.Shotgun.Mati_JakobsRare_SG").unwrap();
    assert_eq!(inherited.chain.len(), 3);
    assert_eq!(inherited.channel("p_WeapClassSelect"), Some(1));

    // The mask texture: large mips in the texture cache, small ones in the package.
    let index = pkg.export_index("Weap_Pistol.Tex.Weap_Pistols_Comp").unwrap();
    let texture = Texture::from_export(pkg, index).unwrap();
    let sizes: Vec<(u32, bool)> = texture.mips.iter().map(|m| (m.width, m.is_inline())).collect();
    println!("Weap_Pistols_Comp mips: {sizes:?}");
    assert_eq!(sizes[0], (2048, false));
    assert!(sizes.contains(&(64, true)));
    for size in [64, 256] {
        let pixels = texture.decode(texture.mip_for(size).unwrap(), &dir).unwrap();
        assert_eq!((pixels.width, pixels.height, pixels.rgba.len()), (size, size, (size * size * 4) as usize));
        // The lower half is the zone map: almost every texel is close to a pure red, green or blue.
        let lower = &pixels.rgba[pixels.rgba.len() / 2..];
        let pure = lower.as_chunks::<4>().0.iter().filter(|p| p[..3].iter().filter(|&&c| c > 128).count() == 1).count();
        let share = pure as f32 / (lower.len() / 4) as f32;
        println!("  {size}px: {:.0}% of the lower half is one pure channel", share * 100.0);
        assert!(share > 0.8, "{share}");
    }
}

#[test]
fn timings() {
    let Some(install) = install() else { return };
    let t = Instant::now();
    let mut pictures = Pictures::open(&install).unwrap();
    let opened = t.elapsed();
    let mut line = format!("open {opened:.0?}");
    for (width, height) in [(1120, 560), (240, 120)] {
        for run in ["first", "again"] {
            let t = Instant::now();
            let png = pictures.weapon_png(INFINITY, width, height).unwrap();
            line += &format!("; {width}x{height} {run} {:.0?} ({} bytes)", t.elapsed(), png.len());
        }
    }
    println!("{line} (test build)");
    // Generous limits: this is a debug-profile test build.
    assert!(opened.as_secs() < 30);
}

/// The pattern layer: the common Jakobs, Vladof and Dahl skins name a full-colour pattern
/// texture that reads from the texture cache, and drawing with it changes the picture.
#[test]
fn pattern_layers_of_common_skins() {
    let (Some(pkg), Some(pictures)) = (package(), pictures()) else { return };
    let dir = pictures.source().package_dir().to_path_buf();
    for (name, texture) in [
        ("Pistol.Mati_JakobsCommonPistol", "Pattern_JakobsCommon"),
        ("Pistol.Mati_VladofCommon", "VladovMetalCommon_Dif"),
        ("Pistol.Mati_DahlCommon", "Pattern_DahlCommon"),
        ("SMG.Mati_MaliwanRareSMG", "Pattern_HexScrollGrunge_Comp"),
    ] {
        let params = MaterialParameters::read(pkg, &format!("Common_GunMaterials.Materials.{name}")).unwrap();
        let path = gamemesh::material::GunSkin::pattern_path(&params).unwrap_or_else(|| panic!("{name}: no pattern"));
        assert_eq!(path.rsplit('.').next(), Some(texture), "{name}");
        let mask = params.channel_mask("p_PatternChannel").unwrap();
        // Full-colour patterns select R, G and B; the packed "_Comp" textures one channel.
        assert_eq!(mask[..3].iter().all(|&on| on), !texture.ends_with("_Comp"), "{name}: {mask:?}");
        let texture = pictures.source().texture(path).unwrap_or_else(|| panic!("{name}: {path} was not read"));
        let mip = texture.mip_for(256).unwrap();
        let pixels = texture.decode(mip, &dir).unwrap();
        assert_eq!((pixels.width, pixels.height), (256, 256), "{name}");
        println!("{name}: {path} scale {:?} strength {:?}", params.vector("p_PatternScalePosition"), params.vector("p_PatternChannelScale"));
    }
    // A Jakobs common pistol: with the wood pattern the picture is browner than without it.
    let p = "GD_Weap_Pistol";
    let parts = [
        Some(format!("{p}.Body.Pistol_Body_Jakobs")),
        Some(format!("{p}.Grip.Pistol_Grip_Jakobs")),
        Some(format!("{p}.Barrel.Pistol_Barrel_Jakobs")),
        None,
        None,
        None,
        None,
        None,
        Some(format!("{p}.ManufacturerMaterials.Mat_Jakobs_1")),
    ];
    let parts: [Option<&str>; 9] = std::array::from_fn(|i| parts[i].as_deref());
    let assembly = pictures.source().assemble_paths(&format!("{p}.A_Weapons.WeaponType_Jakobs_Pistol"), &parts).unwrap();
    let source = pictures.source();
    let params = source.material(assembly.material.as_deref().unwrap()).unwrap();
    let load = |name: &str, size: u32| {
        let texture = source.texture(name).unwrap();
        std::sync::Arc::new(texture.decode(texture.mip_for(size).unwrap(), &dir).unwrap())
    };
    let (masks, diffuse) = gamemesh::material::GunSkin::texture_paths(params).unwrap();
    let plain = gamemesh::material::GunSkin::new(params, load(masks, 512), load(diffuse, 512), gamemesh::material::SkinDetail::Textured);
    let patterned = gamemesh::material::GunSkin::new(params, load(masks, 512), load(diffuse, 512), gamemesh::material::SkinDetail::Textured)
        .with_pattern(params, load(gamemesh::material::GunSkin::pattern_path(params).unwrap(), 256));
    let gestalt = source.gestalt(&assembly.gestalt).unwrap();
    let (scene, _) = gamemesh::picture::weapon_scene(gestalt, &assembly);
    let view = gamemesh::render::View { width: 400, height: 200, ..Default::default() };
    let warmth = |image: &gamemesh::render::Image| {
        let (mut r, mut b) = (0u64, 0u64);
        for px in image.rgba.as_chunks::<4>().0.iter().filter(|px| px[3] == 255) {
            r += u64::from(px[0]);
            b += u64::from(px[2]);
        }
        r as f64 / b.max(1) as f64
    };
    let with = gamemesh::render::render_with(&scene, &view, &patterned);
    let without = gamemesh::render::render_with(&scene, &view, &plain);
    println!("Jakobs common pistol red/blue: with the wood pattern {:.3}, without {:.3}", warmth(&with), warmth(&without));
    assert!(warmth(&with) > warmth(&without) + 0.01);
    assert_ne!(with.rgba, without.rgba);
}
