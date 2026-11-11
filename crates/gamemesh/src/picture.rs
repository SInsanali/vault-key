//! Turning an assembled weapon into a scene for the renderer.

use crate::gestalt::{Fragment, GestaltMesh};
use crate::render::{Scene, Triangle};
use crate::weapon::Assembly;

/// Placeholder colours per part slot (body, grip, barrel, sight, stock, elemental, accessory 1,
/// accessory 2), used until colours come from the weapon's material.
pub const SLOT_COLOURS: [[u8; 3]; 8] = [
    [182, 184, 192],
    [128, 112, 100],
    [156, 160, 172],
    [104, 108, 122],
    [150, 130, 108],
    [232, 150, 60],
    [214, 180, 96],
    [214, 180, 96],
];

fn push_fragment(scene: &mut Scene, gestalt: &GestaltMesh, name: &str, group: u16) -> bool {
    let mut found = false;
    for fragment in gestalt.fragments_named(name) {
        found = true;
        push_run(scene, gestalt, fragment, group);
    }
    found
}

fn push_run(scene: &mut Scene, gestalt: &GestaltMesh, fragment: &Fragment, group: u16) {
    let vertices = &gestalt.mesh.vertices;
    for tri in gestalt.indices_of(fragment).as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (vertices.get(tri[0] as usize), vertices.get(tri[1] as usize), vertices.get(tri[2] as usize)) else {
            continue;
        };
        let v = [*a, *b, *c];
        let second = |i: u32| gestalt.mesh.second_uv(i as usize);
        let uvs = [0, 1, 2].map(|k| {
            let (a, b) = (v[k].uv, second(tri[k]));
            [a[0], a[1], b[0], b[1]]
        });
        scene.triangles.push(Triangle { positions: v.map(|v| v.position), normals: v.map(|v| v.normal), uvs, group });
    }
}

/// The triangles of an assembled weapon, one colour group per part slot. Also returns the
/// fragment names the gestalt mesh does not have.
pub fn weapon_scene(gestalt: &GestaltMesh, assembly: &Assembly) -> (Scene, Vec<String>) {
    let mut scene = Scene { triangles: Vec::new(), palette: SLOT_COLOURS.to_vec() };
    let mut missing = Vec::new();
    for (slot, name) in assembly.fragments() {
        let group = slot.min(SLOT_COLOURS.len() - 1) as u16;
        if !push_fragment(&mut scene, gestalt, name, group) {
            missing.push(name.to_string());
        }
    }
    (scene, missing)
}

/// Every fragment of a gestalt mesh at once, each in its own colour group.
pub fn whole_gestalt_scene(gestalt: &GestaltMesh) -> Scene {
    let mut scene = Scene::default();
    for (i, fragment) in gestalt.fragments.iter().enumerate() {
        // Spread hues around the colour wheel so neighbouring parts differ.
        let hue = (i as f32 * 0.618_034).fract() * 6.0;
        let x = 1.0 - (hue % 2.0 - 1.0).abs();
        let (r, g, b) = match hue as u32 {
            0 => (1.0, x, 0.0),
            1 => (x, 1.0, 0.0),
            2 => (0.0, 1.0, x),
            3 => (0.0, x, 1.0),
            4 => (x, 0.0, 1.0),
            _ => (1.0, 0.0, x),
        };
        scene.palette.push([r, g, b].map(|c: f32| (110.0 + 120.0 * c) as u8));
        push_run(&mut scene, gestalt, fragment, i as u16);
    }
    scene
}
