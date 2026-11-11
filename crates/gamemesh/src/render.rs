//! A small software renderer: orthographic projection, z-buffer, banded ("toon") shading and
//! ink outlines found from depth, normal and part edges. No GPU and no dependencies.

/// A triangle to draw. `group` indexes [`Scene::palette`] and also separates parts for
/// outlining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub positions: [[f32; 3]; 3],
    pub normals: [[f32; 3]; 3],
    /// Two texture-coordinate sets per corner: (u0, v0, u1, v1).
    pub uvs: [[f32; 4]; 3],
    pub group: u16,
}

/// What a [`Shader`] is told about one visible point of the scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub group: u16,
    /// Interpolated texture coordinates: (u0, v0, u1, v1).
    pub uv: [f32; 4],
    /// The diffuse light term, −1…1, for a light from the upper left in front.
    pub light: f32,
    /// The light band the point falls in: 0 lit, 1 half-lit, 2 shadow.
    pub band: usize,
    /// The surface normal, unit length, turned to face the viewer.
    pub normal: [f32; 3],
}

/// Decides the colour of each visible point. Outlines are drawn over the result.
pub trait Shader {
    fn shade(&self, sample: &Sample) -> [u8; 3];
}

/// Brightness of the three light bands, lit to shadow.
pub const BAND_BRIGHTNESS: [f32; 3] = [1.0, 0.80, 0.58];

/// Flat colour per group from a palette, darkened by light band.
pub struct PaletteShader<'a>(pub &'a [[u8; 3]]);

impl Shader for PaletteShader<'_> {
    fn shade(&self, sample: &Sample) -> [u8; 3] {
        let base = self.0.get(usize::from(sample.group)).copied().unwrap_or([200, 200, 200]);
        base.map(|v| (f32::from(v) * BAND_BRIGHTNESS[sample.band]).round().clamp(0.0, 255.0) as u8)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub triangles: Vec<Triangle>,
    /// Base colour of each group, as RGB.
    pub palette: Vec<[u8; 3]>,
}

/// How to look at the scene. The model's long axis is −Y (muzzle), +Z is up.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub width: u32,
    pub height: u32,
    /// Turn of the camera around the vertical axis, in degrees. 0 is a pure side view.
    pub yaw: f32,
    /// How far the camera is raised above the side view, in degrees.
    pub pitch: f32,
    /// True draws the muzzle towards the right (the weapon's right side is seen).
    pub muzzle_right: bool,
    /// Empty border on each side, as a fraction of the image.
    pub margin: f32,
    /// Samples per pixel along each axis.
    pub supersample: u32,
    /// Background colour; `None` leaves the background transparent.
    pub background: Option<[u8; 3]>,
    /// Ink colour for outlines.
    pub ink: [u8; 3],
    /// Outline width in output pixels. 0 draws no outlines.
    pub line_width: f32,
    /// A light rim drawn just outside the silhouette, so that a dark weapon and its ink
    /// outline still read on a dark page. `None` inks the outside of the silhouette instead.
    pub rim: Option<[u8; 3]>,
    /// Fit the view to this box instead of to the triangles (so that several renders of one
    /// weapon type share a scale).
    pub fit_box: Option<([f32; 3], [f32; 3])>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            width: 800,
            height: 400,
            yaw: 0.0,
            pitch: 0.0,
            muzzle_right: false,
            margin: 0.06,
            supersample: 3,
            background: None,
            ink: [24, 22, 30],
            line_width: 1.6,
            rim: None,
            fit_box: None,
        }
    }
}

/// Largest image side drawn; larger requests are clamped.
pub const MAX_SIDE: u32 = 4096;

/// An RGBA image, rows top to bottom, straight (not premultiplied) alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

type V3 = [f32; 3];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn normalize(a: V3) -> V3 {
    let len = dot(a, a).sqrt();
    if len > 1e-12 {
        scale(a, 1.0 / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// The camera's axes in model space: screen right, screen up, and towards the viewer.
fn camera_axes(view: &View) -> (V3, V3, V3) {
    let side = if view.muzzle_right { -1.0 } else { 1.0 };
    let (yaw, pitch) = (view.yaw.to_radians() * side, view.pitch.to_radians());
    // Side view from the −X side: right is +Y, up is +Z, the viewer is at −X.
    let right = [-yaw.sin() * side, yaw.cos() * side, 0.0];
    let toward0 = [-yaw.cos() * side, -yaw.sin() * side, 0.0];
    let up0 = [0.0, 0.0, 1.0];
    let toward = add(scale(toward0, pitch.cos()), scale(up0, pitch.sin()));
    let up = add(scale(up0, pitch.cos()), scale(toward0, -pitch.sin()));
    (right, up, toward)
}

const EMPTY: u16 = u16::MAX;

/// Whether every coordinate of a triangle is a finite number (file data may hold anything).
fn is_finite(t: &Triangle) -> bool {
    t.positions.iter().flatten().all(|v| v.is_finite())
}

/// The diffuse term above which a point is in band 0 (lit) and band 1 (half-lit).
const BAND_THRESHOLDS: [f32; 2] = [0.62, 0.18];

/// Draws the scene with its palette. An empty scene gives an empty (background-only) image.
pub fn render(scene: &Scene, view: &View) -> Image {
    render_with(scene, view, &PaletteShader(&scene.palette))
}

/// Draws the scene, asking `shader` for the colour of every visible point.
pub fn render_with(scene: &Scene, view: &View, shader: &dyn Shader) -> Image {
    let ss = view.supersample.clamp(1, 8) as usize;
    let (out_w, out_h) = (view.width.clamp(1, MAX_SIDE) as usize, view.height.clamp(1, MAX_SIDE) as usize);
    let (w, h) = (out_w * ss, out_h * ss);
    let (right, up, toward) = camera_axes(view);
    let project = |p: V3| -> V3 { [dot(p, right), dot(p, up), dot(p, toward)] };

    // Fit the projected bounds into the image.
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut grow = |p: V3| {
        let q = project(p);
        for a in 0..2 {
            lo[a] = lo[a].min(q[a]);
            hi[a] = hi[a].max(q[a]);
        }
    };
    match view.fit_box {
        Some((a, b)) => {
            for corner in 0..8 {
                grow([
                    if corner & 1 == 0 { a[0] } else { b[0] },
                    if corner & 2 == 0 { a[1] } else { b[1] },
                    if corner & 4 == 0 { a[2] } else { b[2] },
                ]);
            }
        }
        None => scene.triangles.iter().filter(|t| is_finite(t)).flat_map(|t| t.positions).for_each(&mut grow),
    }
    let mut depth = vec![f32::NEG_INFINITY; w * h];
    let mut group = vec![EMPTY; w * h];
    let mut normal = vec![[0f32; 3]; w * h];
    let mut uv = vec![[0f32; 4]; w * h];

    if lo[0].is_finite() {
        let margin = view.margin.clamp(0.0, 0.45);
        let span = [(hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6)];
        let zoom = (w as f32 * (1.0 - 2.0 * margin) / span[0]).min(h as f32 * (1.0 - 2.0 * margin) / span[1]);
        let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let to_screen = |p: V3| -> V3 {
            let q = project(p);
            [w as f32 / 2.0 + (q[0] - centre[0]) * zoom, h as f32 / 2.0 - (q[1] - centre[1]) * zoom, q[2]]
        };

        for t in scene.triangles.iter().filter(|t| is_finite(t)) {
            let [a, b, c] = t.positions.map(to_screen);
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-9 {
                continue;
            }
            let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let max_x = (a[0].max(b[0]).max(c[0]).ceil() as isize).min(w as isize - 1);
            let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let max_y = (a[1].max(b[1]).max(c[1]).ceil() as isize).min(h as isize - 1);
            if max_x < 0 || max_y < 0 {
                continue;
            }
            for y in min_y..=max_y as usize {
                let py = y as f32 + 0.5;
                for x in min_x..=max_x as usize {
                    let px = x as f32 + 0.5;
                    // Barycentric weights.
                    let w0 = ((b[0] - px) * (c[1] - py) - (b[1] - py) * (c[0] - px)) / area;
                    let w1 = ((c[0] - px) * (a[1] - py) - (c[1] - py) * (a[0] - px)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                    let at = y * w + x;
                    if z > depth[at] {
                        depth[at] = z;
                        group[at] = t.group;
                        normal[at] = add(add(scale(t.normals[0], w0), scale(t.normals[1], w1)), scale(t.normals[2], w2));
                        for (k, value) in uv[at].iter_mut().enumerate() {
                            *value = w0 * t.uvs[0][k] + w1 * t.uvs[1][k] + w2 * t.uvs[2][k];
                        }
                    }
                }
            }
        }
    }

    // Shade: bands of a single directional light from the upper left, in front.
    let light = normalize(add(add(scale(right, -0.35), scale(up, 0.75)), scale(toward, 0.55)));
    let mut colour = vec![[0u8; 4]; w * h];
    for at in 0..w * h {
        if group[at] == EMPTY {
            continue;
        }
        let mut n = normalize(normal[at]);
        if dot(n, toward) < 0.0 {
            n = scale(n, -1.0); // shade both sides
        }
        normal[at] = n;
        let diffuse = dot(n, light);
        let band = BAND_THRESHOLDS.iter().position(|&threshold| diffuse > threshold).unwrap_or(2);
        let c = shader.shade(&Sample { group: group[at], uv: uv[at], light: diffuse, band, normal: n });
        colour[at] = [c[0], c[1], c[2], 255];
    }

    // Ink: a sample is inked when a neighbour a line-width away is across an edge.
    if view.line_width > 0.0 {
        let reach = ((view.line_width * ss as f32) / 2.0).round().max(1.0) as isize;
        // A depth step counts as an edge above this many model units.
        let depth_step = 0.012 * (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0);
        let ink = [view.ink[0], view.ink[1], view.ink[2], 255];
        let outside = view.rim.map_or(ink, |c| [c[0], c[1], c[2], 255]);
        let mut inked = vec![false; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                let at = y as usize * w + x as usize;
                let here = group[at];
                let mut edge = false;
                // Outside the shape the rim reaches at least one whole output pixel.
                let reach = if here == EMPTY && view.rim.is_some() { reach.max(ss as isize) } else { reach };
                for (dx, dy) in [(reach, 0), (-reach, 0), (0, reach), (0, -reach)] {
                    let (nx, ny) = (x + dx, y + dy);
                    let there = if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        None
                    } else {
                        Some(ny as usize * w + nx as usize)
                    };
                    let there_group = there.map_or(EMPTY, |i| group[i]);
                    edge |= match (here == EMPTY, there_group == EMPTY) {
                        (true, true) => false,
                        // The silhouette, drawn on both sides of the boundary.
                        (true, false) | (false, true) => true,
                        (false, false) => {
                            let i = there.unwrap_or(at);
                            // Drawn on the farther surface, so nearer shapes keep their size.
                            (depth[i] - depth[at] > depth_step)
                                || (there_group != here && depth[i] >= depth[at])
                                || dot(normal[i], normal[at]) < 0.35
                        }
                    };
                }
                inked[at] = edge;
            }
        }
        for at in 0..w * h {
            if inked[at] {
                colour[at] = if group[at] == EMPTY { outside } else { ink };
            }
        }
    }

    // Box-filter down to the output size.
    let (ow, oh) = (out_w, out_h);
    let mut rgba = Vec::with_capacity(ow * oh * 4);
    let samples = (ss * ss) as f32;
    for oy in 0..oh {
        for ox in 0..ow {
            let mut sum = [0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let c = colour[(oy * ss + sy) * w + ox * ss + sx];
                    let alpha = f32::from(c[3]);
                    sum[0] += f32::from(c[0]) * alpha;
                    sum[1] += f32::from(c[1]) * alpha;
                    sum[2] += f32::from(c[2]) * alpha;
                    sum[3] += alpha;
                }
            }
            let coverage = sum[3] / (255.0 * samples);
            let rgb = if sum[3] > 0.0 { [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3]] } else { [0.0; 3] };
            match view.background {
                Some(bg) => {
                    for a in 0..3 {
                        rgba.push((rgb[a] * coverage + f32::from(bg[a]) * (1.0 - coverage)).round() as u8);
                    }
                    rgba.push(255);
                }
                None => {
                    rgba.extend(rgb.map(|v| v.round() as u8));
                    rgba.push((coverage * 255.0).round() as u8);
                }
            }
        }
    }
    Image { width: ow as u32, height: oh as u32, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y0: f32, y1: f32, z0: f32, z1: f32, x: f32, group: u16) -> [Triangle; 2] {
        let n = [[-1.0, 0.0, 0.0]; 3];
        let (a, b, c, d) = ([x, y0, z0], [x, y1, z0], [x, y1, z1], [x, y0, z1]);
        let uvs = [[0.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [1.0, 1.0, 0.0, 0.0]];
        [
            Triangle { positions: [a, b, c], normals: n, uvs, group },
            Triangle { positions: [a, c, d], normals: n, uvs: [uvs[0], uvs[2], [0.0, 1.0, 0.0, 0.0]], group },
        ]
    }

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        [image.rgba[at], image.rgba[at + 1], image.rgba[at + 2], image.rgba[at + 3]]
    }

    fn plain_view(width: u32, height: u32) -> View {
        View { width, height, margin: 0.25, supersample: 2, line_width: 0.0, ..View::default() }
    }

    #[test]
    fn empty_scene_gives_a_blank_image() {
        let image = render(&Scene::default(), &plain_view(8, 4));
        assert_eq!((image.width, image.height, image.rgba.len()), (8, 4, 8 * 4 * 4));
        assert!(image.rgba.iter().all(|&b| b == 0));
        let image = render(&Scene::default(), &View { background: Some([1, 2, 3]), ..plain_view(2, 2) });
        assert_eq!(pixel(&image, 1, 1), [1, 2, 3, 255]);
    }

    #[test]
    fn a_quad_fills_the_middle_and_leaves_the_margin() {
        let scene = Scene { triangles: quad(-10.0, 10.0, -5.0, 5.0, 0.0, 0).to_vec(), palette: vec![[200, 100, 50]] };
        let image = render(&scene, &plain_view(80, 40));
        // The quad is twice as wide as tall, like the image, so it fills the inner half.
        assert_eq!(pixel(&image, 40, 20)[3], 255);
        assert_eq!(pixel(&image, 22, 12)[3], 255);
        assert_eq!(pixel(&image, 57, 27)[3], 255);
        assert_eq!(pixel(&image, 17, 20)[3], 0);
        assert_eq!(pixel(&image, 40, 7)[3], 0);
        // One flat colour: the base colour times one of the light bands.
        let c = pixel(&image, 40, 20);
        assert!(BAND_BRIGHTNESS.iter().any(|b| c[0] == (200.0 * b).round() as u8), "{c:?}");
        assert_eq!(pixel(&image, 30, 15), c);
    }

    #[test]
    fn nearer_surfaces_hide_farther_ones_whatever_the_order() {
        // The viewer is on the −X side, so x = −1 is in front of x = +1.
        let near = quad(-10.0, 10.0, -5.0, 5.0, -1.0, 0);
        let far = quad(-10.0, 10.0, -5.0, 5.0, 1.0, 1);
        let palette = vec![[255, 0, 0], [0, 0, 255]];
        for triangles in [[near, far].concat(), [far, near].concat()] {
            let image = render(&Scene { triangles, palette: palette.clone() }, &plain_view(40, 20));
            let c = pixel(&image, 20, 10);
            assert!(c[0] > 0 && c[2] == 0, "{c:?}");
        }
    }

    #[test]
    fn shaders_see_interpolated_texture_coordinates() {
        struct ShowUv;
        impl Shader for ShowUv {
            fn shade(&self, s: &Sample) -> [u8; 3] {
                let _ = s.normal;
                [(s.uv[0] * 255.0) as u8, (s.uv[1] * 255.0) as u8, s.band as u8]
            }
        }
        // The quad's u runs along −Y → +Y (left to right), v along −Z → +Z (bottom to top).
        let scene = Scene { triangles: quad(-10.0, 10.0, -5.0, 5.0, 0.0, 0).to_vec(), palette: Vec::new() };
        let image = render_with(&scene, &plain_view(80, 40), &ShowUv);
        let (left, right) = (pixel(&image, 24, 20), pixel(&image, 56, 20));
        let (top, bottom) = (pixel(&image, 40, 12), pixel(&image, 40, 28));
        assert!(left[0] < 40 && right[0] > 215, "{left:?} {right:?}");
        assert!(bottom[1] < 60 && top[1] > 195, "{top:?} {bottom:?}");
        assert!(left[2] <= 2);
    }

    #[test]
    fn rim_and_odd_input() {
        let scene = Scene { triangles: quad(-10.0, 10.0, -4.0, 4.0, 0.0, 0).to_vec(), palette: vec![[40, 40, 40]] };
        let view = View { line_width: 2.0, ink: [1, 2, 3], rim: Some([250, 240, 200]), ..plain_view(100, 40) };
        let image = render(&scene, &view);
        // Going up the middle column from outside: transparent, then light rim, then dark ink.
        let column: Vec<[u8; 4]> = (0..20).map(|y| pixel(&image, 50, y)).collect();
        let first_rim = column.iter().position(|c| c[3] > 200 && c[0] > 200).expect("a rim");
        let first_ink = column.iter().position(|c| c[3] == 255 && c[0] < 30).expect("ink");
        assert!(column[0][3] == 0 && first_rim < first_ink, "{column:?}");

        // Non-finite corners are skipped, not drawn and not a panic; absurd sizes are clamped.
        let mut bad = quad(-10.0, 10.0, -4.0, 4.0, 0.0, 0).to_vec();
        bad[0].positions[0][1] = f32::NAN;
        bad[1].positions[2][2] = f32::INFINITY;
        let image = render(&Scene { triangles: bad, palette: Vec::new() }, &plain_view(20, 10));
        assert!(image.rgba.iter().all(|&b| b == 0));
        let image = render(&Scene::default(), &View { width: 0, height: u32::MAX, supersample: 99, ..plain_view(1, 1) });
        assert_eq!((image.width, image.height), (1, MAX_SIDE));
    }

    #[test]
    fn muzzle_direction_and_outline() {
        // A small marker quad at the muzzle end (−Y) of a long body.
        let mut triangles = quad(-10.0, 10.0, -1.0, 1.0, 0.0, 0).to_vec();
        triangles.extend(quad(-10.0, -8.0, 1.0, 3.0, 0.0, 1));
        let scene = Scene { triangles, palette: vec![[90, 90, 90], [250, 250, 250]] };
        let left = render(&scene, &plain_view(100, 40));
        let right = render(&scene, &View { muzzle_right: true, ..plain_view(100, 40) });
        let has_marker = |image: &Image, x0: u32, x1: u32| (x0..x1).any(|x| (0..40).any(|y| pixel(image, x, y)[0] > 130));
        assert!(has_marker(&left, 0, 50) && !has_marker(&left, 50, 100));
        assert!(has_marker(&right, 50, 100) && !has_marker(&right, 0, 50));

        // With outlines, the silhouette boundary is ink and the interior is not.
        let view = View { line_width: 2.0, ink: [1, 2, 3], ..plain_view(100, 40) };
        let inked = render(&Scene { triangles: quad(-10.0, 10.0, -4.0, 4.0, 0.0, 0).to_vec(), palette: vec![[200, 200, 200]] }, &view);
        assert!(pixel(&inked, 50, 20)[0] > 100);
        let column: Vec<[u8; 4]> = (0..40).map(|y| pixel(&inked, 50, y)).collect();
        assert!(column.iter().any(|c| c[3] == 255 && c[0] < 10), "no ink on the silhouette");
    }
}
