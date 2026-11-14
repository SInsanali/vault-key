//! Turning an assembled weapon into a scene for the renderer.

use crate::error::{MeshError, Result};
use crate::gestalt::{Fragment, GestaltMesh};
use crate::render::{Scene, Triangle};
use crate::skeletal::MAX_TRIANGLES;
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
///
/// Parts may name the same fragment, and a name may have several runs, so the scene can hold
/// more triangles than the mesh. More than [`MAX_TRIANGLES`] in all is
/// [`MeshError::Unsupported`], found before any is copied.
pub fn weapon_scene(gestalt: &GestaltMesh, assembly: &Assembly) -> Result<(Scene, Vec<String>)> {
    let total: u64 = assembly
        .fragments()
        .flat_map(|(_, name)| gestalt.fragments_named(name))
        .map(|fragment| u64::from(fragment.triangle_count))
        .sum();
    if total > MAX_TRIANGLES as u64 {
        return Err(MeshError::Unsupported(format!(
            "{}: {total} triangles in the picture, more than {MAX_TRIANGLES}",
            gestalt.definition_path
        )));
    }
    let mut scene = Scene { triangles: Vec::new(), palette: SLOT_COLOURS.to_vec() };
    let mut missing = Vec::new();
    for (slot, name) in assembly.fragments() {
        let group = slot.min(SLOT_COLOURS.len() - 1) as u16;
        if !push_fragment(&mut scene, gestalt, name, group) {
            missing.push(name.to_string());
        }
    }
    Ok((scene, missing))
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

/// Turns a one-piece model (a class mod) that the camera would see edge-on so that it sees
/// its face. The camera looks along x and shows y and z; when the y-z outline is a sliver
/// (Krieg's saw blade stands in the x-z plane), the thinnest side goes to x and the longest
/// across the picture. A model whose outline is already a proper shape is left alone: Gaige's
/// is a thick box whose decorated side is the one drawn now. A proper rotation, never a
/// mirror, so lettering still reads the right way round.
pub fn face_camera(scene: &mut Scene) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in scene.triangles.iter().flat_map(|t| t.positions.iter()) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = [0, 1, 2].map(|k| hi[k] - lo[k]);
    if extent.iter().any(|e| !e.is_finite()) {
        return;
    }
    // Already facing the camera well enough.
    if extent[1].min(extent[2]) >= 0.25 * extent[1].max(extent[2]) {
        return;
    }
    let thin = (0..3).min_by(|&a, &b| extent[a].total_cmp(&extent[b])).unwrap_or(0);
    if thin == 0 {
        return;
    }
    let long = (0..3).filter(|&k| k != thin).max_by(|&a, &b| extent[a].total_cmp(&extent[b])).unwrap_or(1);
    let mid = 3 - thin - long;
    // New (x, y, z) from old axes (thin, long, mid). An odd permutation would mirror the
    // model; turning x round as well makes it a rotation.
    let odd = [(0, 2, 1), (1, 0, 2), (2, 1, 0)].contains(&(thin, long, mid));
    let sign = if odd { -1.0 } else { 1.0 };
    let turn = |v: [f32; 3]| [v[thin] * sign, v[long], v[mid]];
    for triangle in &mut scene.triangles {
        triangle.positions = triangle.positions.map(turn);
        triangle.normals = triangle.normals.map(turn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(corners: [[f32; 3]; 3]) -> Scene {
        let normal = {
            let (a, b, c) = (corners[0], corners[1], corners[2]);
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        };
        Scene { triangles: vec![Triangle { positions: corners, normals: [normal; 3], uvs: [[0.0; 4]; 3], group: 0 }], palette: Vec::new() }
    }

    fn cross_matches_normal(t: &Triangle) -> bool {
        let [a, b, c] = t.positions;
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        n.iter().zip(t.normals[0]).map(|(a, b)| a * b).sum::<f32>() > 0.0
    }

    /// A card standing in the x-z plane is seen edge-on; it is turned to face the camera
    /// (thin along x), by a rotation: the winding still agrees with the normal.
    #[test]
    fn edge_on_models_are_turned_to_face_the_camera() {
        let mut scene = card([[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.2, 8.0]]);
        face_camera(&mut scene);
        let t = &scene.triangles[0];
        let extent = |k: usize| {
            let values = t.positions.map(|p| p[k]);
            values.iter().copied().fold(f32::MIN, f32::max) - values.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(extent(0) < extent(1) && extent(0) < extent(2), "{:?}", t.positions);
        assert!(extent(1) >= extent(2), "the longest side runs across the picture");
        assert!(cross_matches_normal(t), "turned, not mirrored");
    }

    #[test]
    fn models_already_facing_the_camera_are_left_alone() {
        // A thick box seen from its side: a proper outline in y-z.
        let original = card([[0.0, 0.0, 0.0], [6.0, 10.0, 0.0], [3.0, 0.0, 5.0]]);
        let mut scene = original.clone();
        face_camera(&mut scene);
        assert_eq!(scene.triangles[0].positions, original.triangles[0].positions);
    }
}
