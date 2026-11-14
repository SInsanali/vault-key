//! Weapon materials: the parameters of a `MaterialInstanceConstant` chain, and an experimental
//! shader that colours a weapon from them.
//!
//! What is read from the data is solid (parameter names and values, which textures, which
//! channel). How the game's shader combines them is NOT in the cooked packages (the material
//! graph is stripped), so [`GunSkin`]'s formula is an approximation chosen by eye. See the
//! research document, §5.

use std::collections::BTreeMap;
use std::sync::Arc;

use gamedata::package::{PropertyList, PropertyValue, UPackage};

use crate::error::{MeshError, Result};
use crate::render::{Sample, Shader, BAND_BRIGHTNESS};
use crate::texture::Pixels;

/// Longest parent chain followed.
const MAX_CHAIN: usize = 8;

/// The parameters of a material instance, with every parent instance's values filled in
/// underneath (a child's value wins). Names are lower-cased: the engine compares names without
/// regard to case, and the data mixes `p_` and `P_`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialParameters {
    /// The instance and its parents, nearest first, ending with the base `Material`.
    pub chain: Vec<String>,
    pub scalars: BTreeMap<String, f32>,
    /// Linear RGBA; values above 1 are common.
    pub vectors: BTreeMap<String, [f32; 4]>,
    /// Texture object paths.
    pub textures: BTreeMap<String, String>,
    /// Static channel masks (R, G, B, A), such as `p_weapclassselect`.
    pub channel_masks: BTreeMap<String, [bool; 4]>,
}

impl MaterialParameters {
    /// Reads the instance at `path` in `pkg` and its parents (all in the same package).
    pub fn read(pkg: &UPackage, path: &str) -> Result<Self> {
        Self::read_across(std::slice::from_ref(pkg), path)
    }

    /// Reads the instance at `path` and its parents, wherever in `packages` each is (a DLC
    /// skin's parent is often a base-game instance). A parent in none of them ends the chain,
    /// its path kept as the last entry so the family can still be told.
    pub fn read_across(packages: &[UPackage], path: &str) -> Result<Self> {
        let all = crate::weapon::Packages(packages);
        let mut out = Self::default();
        let (mut at, mut index) = all.locate(path).ok_or_else(|| MeshError::Missing(format!("material {path}")))?;
        for _ in 0..MAX_CHAIN {
            let pkg = all.get(at);
            out.chain.push(pkg.export_path(index)?);
            if pkg.class_name_of_export(index) != "MaterialInstanceConstant" {
                break;
            }
            let (props, native) = pkg.properties_and_native_data(index)?;
            out.absorb(pkg, &props)?;
            out.absorb_static_masks(pkg, native);
            let Some(parent) = props.object("Parent").filter(|&r| r != 0) else { break };
            match all.resolve(at, parent) {
                Some(next) => (at, index) = next,
                None => {
                    if let Ok(name) = pkg.path_of(parent) {
                        out.chain.push(name);
                    }
                    break;
                }
            }
        }
        Ok(out)
    }

    fn absorb(&mut self, pkg: &UPackage, props: &PropertyList) -> Result<()> {
        let entries = |key: &str| -> Result<Vec<PropertyList>> {
            props.get(key).map_or(Ok(Vec::new()), |v| Ok(pkg.struct_array(v)?))
        };
        for e in entries("ScalarParameterValues")? {
            if let Some(name) = e.name("ParameterName") {
                self.scalars.entry(name.to_ascii_lowercase()).or_insert(e.float("ParameterValue").unwrap_or(0.0));
            }
        }
        for e in entries("VectorParameterValues")? {
            let (Some(name), Some(PropertyValue::StructRaw { bytes, .. })) = (e.name("ParameterName"), e.get("ParameterValue")) else {
                continue;
            };
            if bytes.len() == 16 {
                let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
                self.vectors.entry(name.to_ascii_lowercase()).or_insert([f(0), f(4), f(8), f(12)]);
            }
        }
        for e in entries("TextureParameterValues")? {
            if let (Some(name), Some(texture)) = (e.name("ParameterName"), e.object("ParameterValue")) {
                if let Ok(path) = pkg.path_of(texture) {
                    self.textures.entry(name.to_ascii_lowercase()).or_insert(path);
                }
            }
        }
        Ok(())
    }

    /// Static parameters sit in the instance's native data: a channel mask as name, four
    /// 32-bit flags (R, G, B, A), an override flag and a GUID; a switch (`sw_…`) as name, its
    /// value, an override flag and a GUID (kept here as `[value, override, false, false]`).
    /// They are found by looking for the parameter names; the block's full layout was not
    /// worked out.
    fn absorb_static_masks(&mut self, pkg: &UPackage, native: &[u8]) {
        for wanted in ["p_WeapClassSelect", "p_DecalChannel", "p_PatternChannel", "sw_FlipDecalOnRightSide"] {
            let Some(name_index) = pkg.names.iter().position(|n| n.eq_ignore_ascii_case(wanted)) else {
                continue;
            };
            let mut needle = (name_index as i32).to_le_bytes().to_vec();
            needle.extend(0i32.to_le_bytes());
            let Some(at) = native.windows(8).position(|w| w == needle) else {
                continue;
            };
            let Some(flags) = native.get(at + 8..at + 28) else {
                continue;
            };
            let word = |i: usize| u32::from_le_bytes([flags[i * 4], flags[i * 4 + 1], flags[i * 4 + 2], flags[i * 4 + 3]]);
            let words = if wanted.starts_with("sw_") { 2 } else { 5 };
            if (0..words).all(|i| word(i) <= 1) {
                let on = |i: usize| i < words - 1 && word(i) == 1;
                self.channel_masks.entry(wanted.to_ascii_lowercase()).or_insert([on(0), on(1), on(2), on(3)]);
            }
        }
    }

    pub fn vector(&self, name: &str) -> Option<[f32; 4]> {
        self.vectors.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn scalar(&self, name: &str) -> Option<f32> {
        self.scalars.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn texture(&self, name: &str) -> Option<&str> {
        self.textures.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// The first channel a static mask selects (0 = R).
    pub fn channel(&self, name: &str) -> Option<usize> {
        self.channel_masks.get(&name.to_ascii_lowercase())?.iter().position(|&on| on)
    }

    /// A static channel mask as stored: which of R, G, B, A are on.
    pub fn channel_mask(&self, name: &str) -> Option<[bool; 4]> {
        self.channel_masks.get(&name.to_ascii_lowercase()).copied()
    }
}

/// How a layer's texture is read: as a colour, or one channel as a grey mask. From the
/// static mask `p_PatternChannel` / `p_DecalChannel`: all three colour channels on means
/// colour, otherwise the first channel that is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerChannel {
    Colour,
    Grey(usize),
}

impl LayerChannel {
    /// `unset` is what a material without the static mask gets: gun patterns are colour
    /// images (Jakobs wood, Bandit paint), item patterns are packed `_Comp` masks whose red
    /// channel holds the pattern (read as colour, their blue and green turn it to glitter).
    fn from_mask(mask: Option<[bool; 4]>, unset: Self) -> Self {
        match mask {
            None => unset,
            Some([true, true, true, _]) => Self::Colour,
            Some(mask) => Self::Grey(mask.iter().position(|&on| on).unwrap_or(0)),
        }
    }

    fn read(self, texel: [f32; 4]) -> [f32; 3] {
        match self {
            Self::Colour => [texel[0], texel[1], texel[2]],
            Self::Grey(channel) => [texel[channel.min(3)]; 3],
        }
    }
}

/// The pattern layer (`p_Pattern`): a texture tiled over the weapon's side projection
/// (texture-coordinate set 1), laid onto each colour zone by its own strength.
pub struct Pattern {
    pixels: Arc<Pixels>,
    channel: LayerChannel,
    /// `p_PatternScalePosition`: tiling in x and y, then an offset.
    scale: [f32; 2],
    offset: [f32; 2],
    /// `p_PatternChannelScale`: how much of the pattern zones A, B, C and the rest take.
    strength: [f32; 4],
    /// `p_PatternColor`: the colour a grey-mask pattern is drawn in.
    colour: [f32; 3],
    /// `p_ReplacePattern`: the pattern replaces the zone colour instead of multiplying it.
    replace: bool,
}

impl Pattern {
    /// The pattern's colour at a point, and how much of it each zone takes (zero everywhere
    /// when the material has no pattern layer).
    fn at(&self, uv1: [f32; 2]) -> [f32; 3] {
        let u = uv1[0] * self.scale[0] + self.offset[0];
        let v = uv1[1] * self.scale[1] + self.offset[1];
        let texel = self.pixels.sample_linear(u, v).map(|c| c / 255.0);
        // `p_PatternColor` colours a grey mask; a full-colour pattern brings its own colours
        // (with the multiplier, Jakobs wood and Vladof metal come out pale and wrong).
        match self.channel {
            LayerChannel::Colour => self.channel.read(texel),
            LayerChannel::Grey(_) => {
                let c = self.channel.read(texel);
                [c[0] * self.colour[0], c[1] * self.colour[1], c[2] * self.colour[2]]
            }
        }
    }

    fn apply(&self, zone: usize, base: [f32; 3], pattern: [f32; 3]) -> [f32; 3] {
        let strength = self.strength[zone.min(3)].clamp(0.0, 1.0);
        if strength == 0.0 {
            return base;
        }
        let patterned = if self.replace { pattern } else { [base[0] * pattern[0], base[1] * pattern[1], base[2] * pattern[2]] };
        mix(base, patterned, strength)
    }
}

/// The decal layer (`p_Decal`): one image placed on the weapon's side, over everything.
pub struct Decal {
    pixels: Arc<Pixels>,
    channel: LayerChannel,
    /// `p_DecalScalePosition`: scale in x and y, then an offset, applied to coordinate set 1.
    scale: [f32; 2],
    offset: [f32; 2],
    /// `p_DecalColor`; with `p_UseFullColorDecal` a multiplier on the image, else the colour
    /// the mask is drawn in.
    colour: [f32; 3],
    full_colour: bool,
    /// `p_ReplaceDecal`, read as how opaque the decal is.
    opacity: f32,
    /// `sw_FlipDecalOnRightSide`: mirror the decal on the +X side so it reads correctly.
    flip_on_right: bool,
}

impl Decal {
    /// The decal's colour and coverage at a point, or none outside the decal's square.
    fn at(&self, uv1: [f32; 2], normal_x: f32) -> Option<([f32; 3], f32)> {
        let mut u = uv1[0] * self.scale[0] + self.offset[0];
        let v = uv1[1] * self.scale[1] + self.offset[1];
        if self.flip_on_right && normal_x > 0.0 {
            u = 1.0 - u;
        }
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let texel = self.pixels.sample_linear(u, v).map(|c| c / 255.0);
        let (colour, coverage) = match self.channel {
            LayerChannel::Colour if self.full_colour => ([texel[0] * self.colour[0], texel[1] * self.colour[1], texel[2] * self.colour[2]], texel[3]),
            channel => (self.colour, channel.read(texel)[0]),
        };
        Some((colour, (coverage * self.opacity).clamp(0.0, 1.0)))
    }
}

/// How much of the game's material [`GunSkin`] reproduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinDetail {
    /// One flat colour per colour zone (the zone's midtone), plus light bands.
    Flat,
    /// Zone colours shaped by the highlight and shadow masks and the greyscale diffuse.
    Textured,
}

/// Which master material a skin is an instance of, and so how its textures are laid out.
/// The game has one shader per family; the cooked packages keep only their parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkinFamily {
    /// `Master_Gun`: the mask texture is two maps stacked, colour zones below and the
    /// highlight and shadow masks above; `p_Diffuse` is a greyscale drawing shared by three
    /// weapon classes, one per channel (`p_WeapClassSelect`); the fourth colour is `p_DColor`.
    Gun,
    /// `Master_ShieldGrenade` and the class mod masters: the two maps of the mask texture sit
    /// side by side, highlight and shadow masks on the left, colour zones on the right;
    /// `p_Diffuse` is an ordinary colour drawing of the whole layout; the fourth colour is
    /// `p_ColorD`.
    Item,
}

impl SkinFamily {
    /// The family of a material, from the master material its chain ends at. Anything not
    /// recognised is read as a gun skin, as it always was.
    pub fn of(params: &MaterialParameters) -> Self {
        let master = params.chain.last().map(|m| m.to_ascii_lowercase()).unwrap_or_default();
        if master.ends_with(".master_shieldgrenade") || master.contains(".master_classmod") {
            Self::Item
        } else {
            Self::Gun
        }
    }
}

/// An approximation of the game's weapon material.
///
/// The mask texture (`p_Masks`) is two maps stacked: its lower half marks the three colour
/// zones A, B, C in red, green and blue; its upper half holds a highlight mask (red) and a
/// shadow mask (green). Both are addressed by the first texture-coordinate set with v halved.
/// The diffuse texture (`p_Diffuse`) is shared by three weapon classes, one per channel; the
/// static mask `p_WeapClassSelect` says which.
pub struct GunSkin {
    pub detail: SkinDetail,
    pub family: SkinFamily,
    masks: Arc<Pixels>,
    diffuse: Arc<Pixels>,
    diffuse_channel: usize,
    /// Zones A, B, C: highlight, midtone, shadow (linear RGB).
    zones: [[[f32; 3]; 3]; 3],
    /// Colour where no zone is marked.
    rest: [f32; 3],
    highlights: f32,
    shadows: f32,
    pattern: Option<Pattern>,
    decal: Option<Decal>,
}

impl GunSkin {
    /// The pattern texture a material names, if it has a pattern layer.
    pub fn pattern_path(params: &MaterialParameters) -> Option<&str> {
        params
            .texture("p_Pattern")
            .or_else(|| params.texture("p_CustomPattern"))
            .filter(|_| params.vector("p_PatternChannelScale").is_some_and(|s| s.iter().any(|&v| v > 0.0)))
    }

    /// The decal texture a material names, if it has one.
    pub fn decal_path(params: &MaterialParameters) -> Option<&str> {
        params.texture("p_Decal").filter(|_| params.vector("p_DecalScalePosition").is_some())
    }

    /// Adds the pattern layer, with the material's own pattern parameters.
    pub fn with_pattern(mut self, params: &MaterialParameters, pixels: Arc<Pixels>) -> Self {
        let scale_position = params.vector("p_PatternScalePosition").unwrap_or([1.0, 1.0, 0.0, 0.0]).map(sane_signed);
        let strength = params.vector("p_PatternChannelScale").unwrap_or([0.0; 4]).map(sane);
        self.pattern = Some(Pattern {
            pixels,
            channel: LayerChannel::from_mask(
                params.channel_mask("p_PatternChannel"),
                match SkinFamily::of(params) {
                    SkinFamily::Gun => LayerChannel::Colour,
                    SkinFamily::Item => LayerChannel::Grey(0),
                },
            ),
            scale: [scale_position[0], scale_position[1]],
            offset: [scale_position[2], scale_position[3]],
            strength,
            colour: params.vector("p_PatternColor").map_or([1.0; 3], |c| [c[0], c[1], c[2]].map(sane)),
            replace: params.scalar("p_ReplacePattern").unwrap_or(0.0) >= 0.5,
        });
        self
    }

    /// Adds the decal layer, with the material's own decal parameters.
    pub fn with_decal(mut self, params: &MaterialParameters, pixels: Arc<Pixels>) -> Self {
        let scale_position = params.vector("p_DecalScalePosition").unwrap_or([1.0, 1.0, 0.0, 0.0]).map(sane_signed);
        self.decal = Some(Decal {
            pixels,
            channel: LayerChannel::from_mask(params.channel_mask("p_DecalChannel"), LayerChannel::Colour),
            scale: [scale_position[0], scale_position[1]],
            offset: [scale_position[2], scale_position[3]],
            colour: params.vector("p_DecalColor").map_or([1.0; 3], |c| [c[0], c[1], c[2]].map(sane)),
            full_colour: params.scalar("p_UseFullColorDecal").unwrap_or(1.0) >= 0.5,
            opacity: sane(params.scalar("p_ReplaceDecal").unwrap_or(1.0)),
            flip_on_right: params.channel_mask("sw_FlipDecalOnRightSide").is_some_and(|m| m[0]),
        });
        self
    }

    /// The texture paths a material's skin needs: (`p_Masks`, `p_Diffuse`).
    pub fn texture_paths(params: &MaterialParameters) -> Option<(&str, &str)> {
        Some((params.texture("p_Masks")?, params.texture("p_Diffuse")?))
    }

    /// A skin from a material's parameters and its two decoded textures (see
    /// [`GunSkin::texture_paths`]).
    pub fn new(params: &MaterialParameters, masks: Arc<Pixels>, diffuse: Arc<Pixels>, detail: SkinDetail) -> Self {
        let rgb = |name: &str| params.vector(name).map_or([1.0, 1.0, 1.0], |v| [v[0], v[1], v[2]].map(sane));
        let zone = |letter: char| ["Hilight", "Midtone", "Shadow"].map(|tone| rgb(&format!("p_{letter}Color{tone}")));
        let family = SkinFamily::of(params);
        let rest = match family {
            SkinFamily::Gun => "p_DColor",
            SkinFamily::Item => "p_ColorD",
        };
        Self {
            detail,
            family,
            masks,
            diffuse,
            diffuse_channel: params.channel("p_WeapClassSelect").unwrap_or(0),
            zones: [zone('A'), zone('B'), zone('C')],
            rest: rgb(rest),
            highlights: sane(params.scalar("p_HighlightsIntensity").unwrap_or(2.0)),
            shadows: sane(params.scalar("p_ShadowsIntensity").unwrap_or(2.0)),
            pattern: None,
            decal: None,
        }
    }
}

/// A parameter from the file that may be negative (an offset or a scale): finite and bounded.
fn sane_signed(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-1024.0, 1024.0)
    } else {
        0.0
    }
}

/// A parameter from the file as a usable number: finite and not negative.
fn sane(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 64.0)
    } else {
        0.0
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Brings an over-bright colour into range without changing its hue. The data's colours are
/// often above 1 (a white zone can be 1.8 or 3.4); the game's own exposure is not known, and
/// letting them through washes the drawn lines out to plain white.
fn fit(c: [f32; 3]) -> [f32; 3] {
    let peak = c[0].max(c[1]).max(c[2]);
    if peak > 1.0 {
        c.map(|v| v / peak)
    } else {
        c
    }
}

/// Linear light to an sRGB byte.
fn to_srgb(linear: [f32; 3]) -> [u8; 3] {
    linear.map(|v| {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    })
}

impl Shader for GunSkin {
    fn shade(&self, sample: &Sample) -> [u8; 3] {
        let (u, v) = (sample.uv[0], sample.uv[1].clamp(0.0, 0.9999));
        let uv1 = [sample.uv[2], sample.uv[3]];
        // Where the zone map and the detail map are read for this point (see SkinFamily).
        let (zone_at, detail_at) = match self.family {
            SkinFamily::Gun => ((u, 0.5 + v * 0.5), (u, v * 0.5)),
            SkinFamily::Item => {
                let u = u.clamp(0.0, 0.9999);
                ((0.5 + u * 0.5, v), (u * 0.5, v))
            }
        };
        let zone_mask = self.masks.sample(zone_at.0, zone_at.1);
        let weights = [f32::from(zone_mask[0]), f32::from(zone_mask[1]), f32::from(zone_mask[2])].map(|w| w / 255.0);
        let marked = (weights[0] + weights[1] + weights[2]).max(1e-6);
        // How much of the zone's shadow and highlight tones show here.
        let (mut shadow, mut highlight, mut grey) = (0.0, 0.0, 0.5);
        if self.detail == SkinDetail::Textured {
            // Kept half a texel inside its own map, so the other map never bleeds in.
            // (`half` is at most a quarter, so the bounds below are always in order.)
            let detail = match self.family {
                SkinFamily::Gun => {
                    let half = (0.5 / self.masks.height.max(1) as f32).min(0.25);
                    self.masks.sample_linear(detail_at.0, detail_at.1.clamp(half, 0.5 - half))
                }
                SkinFamily::Item => {
                    let half = (0.5 / self.masks.width.max(1) as f32).min(0.25);
                    self.masks.sample_linear(detail_at.0.clamp(half, 0.5 - half), detail_at.1)
                }
            };
            // The intensities read as exponents: a high value keeps the tone to where the
            // mask is strongest (the data gives Jakobs skins a shadow intensity of 8, and
            // their wood would turn blue under any multiplier reading).
            highlight = (detail[0] / 255.0).clamp(0.0, 1.0).powf(self.highlights.max(0.05));
            shadow = (detail[1] / 255.0).clamp(0.0, 1.0).powf(self.shadows.max(0.05));
            let texel = self.diffuse.sample_linear(u, v);
            grey = match self.family {
                SkinFamily::Gun => texel[self.diffuse_channel.min(3)] / 255.0,
                SkinFamily::Item => (0.2126 * texel[0] + 0.7152 * texel[1] + 0.0722 * texel[2]) / 255.0,
            };
        }
        let pattern = self.pattern.as_ref().map(|p| p.at(uv1));
        // Each zone's colour: its tones, then its share of the pattern; then the zones are
        // mixed by the zone map, and where little is marked the fourth colour shows.
        let zone_colour = |zone: usize, tones: &[[f32; 3]; 3]| -> [f32; 3] {
            let mut c = mix(mix(tones[1], tones[2], shadow), tones[0], highlight);
            if let (Some(pattern), Some(colour)) = (&self.pattern, pattern) {
                c = pattern.apply(zone, c, colour);
            }
            c
        };
        let mut colour = [0.0; 3];
        for (zone, (tones, weight)) in self.zones.iter().zip(weights).enumerate() {
            let c = zone_colour(zone, tones);
            for i in 0..3 {
                colour[i] += c[i] * weight / marked;
            }
        }
        let rest = zone_colour(3, &[self.rest; 3]);
        let mut colour = fit(mix(rest, colour, marked.min(1.0)));
        if let Some((decal, coverage)) = self.decal.as_ref().and_then(|d| d.at(uv1, sample.normal[0])) {
            colour = mix(colour, fit(decal), coverage);
        }
        if self.detail == SkinDetail::Textured {
            // The greyscale drawing: mid grey leaves the colour nearly as it is.
            colour = colour.map(|c| c * (0.30 + 1.25 * grey));
        }
        let lit = BAND_BRIGHTNESS[sample.band.min(2)];
        to_srgb(colour.map(|c| c * lit * lit))
    }
}

/// The relic texture. Relics share one base `Material` (`Mat_ItemArtifact`) whose shader
/// graph is stripped from the cooked package, so nothing in the data names the texture it
/// samples; this is the only texture in the relic package.
pub const RELIC_TEXTURE: &str = "Item_Artifacts.Textures.ItemArtifacts_Comp";

/// An approximation of the relic material: the whole relic glows in its type's colour.
///
/// What the data gives: the material is emissive, two-sided and drawn without the game's
/// edge outlines; each relic kind's body part sets `p_EmissiveColor` (Tenacity green,
/// Vitality red, Elemental purple, …). The texture is three masks: a cloudy surface with
/// drawn swirls (red), edge highlights (green) and the glyphs, bars and dots that light up
/// (blue). It is tiled many times over the mesh. How they are combined is chosen by eye.
pub struct RelicSkin {
    texture: Option<Arc<Pixels>>,
    /// The glow colour, brought into range (linear RGB).
    glow: [f32; 3],
}

impl RelicSkin {
    /// The colour a relic glows when its parts set none: `Mat_ItemArtifact`'s own default.
    pub const DEFAULT_GLOW: [f32; 3] = [0.077, 0.232, 1.0];

    pub fn new(glow: [f32; 3], texture: Option<Arc<Pixels>>) -> Self {
        let glow = fit(glow.map(sane));
        // A colour with no light in it would draw a black relic; use the default instead.
        let glow = if glow.iter().all(|&c| c < 0.01) { Self::DEFAULT_GLOW } else { glow };
        Self { texture, glow }
    }
}

impl Shader for RelicSkin {
    fn shade(&self, sample: &Sample) -> [u8; 3] {
        let (surface, edge, glyph) = match &self.texture {
            Some(texture) => {
                let t = texture.sample_linear(sample.uv[0], sample.uv[1]).map(|c| (c / 255.0).clamp(0.0, 1.0));
                (t[0], t[1], t[2])
            }
            None => (0.6, 0.0, 0.0),
        };
        let glow = self.glow;
        // A dark body in the glow colour, shaped by the drawn surface ...
        let body = glow.map(|c| c * (0.18 + 0.55 * surface));
        // ... with its edges and glyphs lit: the glow colour, washed toward white where it
        // burns brightest, as the game's bloom does.
        let hot = glow.map(|c| c * 0.75 + 0.25);
        let lit = mix(mix(body, glow, edge * 0.7), hot, glyph);
        // Emissive: the light bands touch it only a little.
        let band = BAND_BRIGHTNESS[sample.band.min(2)].sqrt();
        to_srgb(lit.map(|c| c * band))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A texture `width` × `height` whose left half is `left` and right half `right`, top half
    /// over bottom half as given.
    fn halves(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Pixels> {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend(texel(x, y));
            }
        }
        Arc::new(Pixels { width, height, rgba })
    }

    fn params(master: &str, rest: &str) -> MaterialParameters {
        let mut p = MaterialParameters { chain: vec!["Mati_Test".into(), master.into()], ..Default::default() };
        for tone in ["Hilight", "Midtone", "Shadow"] {
            p.vectors.insert(format!("p_acolor{tone}").to_ascii_lowercase(), [1.0, 0.0, 0.0, 1.0]);
            p.vectors.insert(format!("p_bcolor{tone}").to_ascii_lowercase(), [0.0, 0.0, 1.0, 1.0]);
            p.vectors.insert(format!("p_ccolor{tone}").to_ascii_lowercase(), [1.0, 1.0, 1.0, 1.0]);
        }
        p.vectors.insert(rest.to_ascii_lowercase(), [0.0, 1.0, 0.0, 1.0]);
        p
    }

    fn sample(u: f32, v: f32) -> Sample {
        Sample { group: 0, uv: [u, v, 0.0, 0.0], light: 1.0, band: 0, normal: [-1.0, 0.0, 0.0] }
    }

    fn strongest(c: [u8; 3]) -> usize {
        (0..3).max_by_key(|&i| c[i]).unwrap()
    }

    #[test]
    fn the_family_comes_from_the_master_material() {
        let of = |master: &str| SkinFamily::of(&params(master, "p_DColor"));
        assert_eq!(of("Common_Materials.Weapons.Master_Gun"), SkinFamily::Gun);
        assert_eq!(of("Common_Materials.Items.Master_ShieldGrenade"), SkinFamily::Item);
        assert_eq!(of("Common_Materials.Items.Master_ClassMod_02"), SkinFamily::Item);
        assert_eq!(of("Item_ClassMods.Mat.Master_ClassMod"), SkinFamily::Item);
        assert_eq!(of("Something.Else"), SkinFamily::Gun);
    }

    /// One mask texture, read both ways. Its left half marks zone B (green), its right half
    /// zone A (red); zone A is drawn red, zone B blue. An item reads the zones from the right
    /// half (red); a gun from the bottom half, here the left half's green (blue).
    #[test]
    fn item_masks_sit_side_by_side_and_gun_masks_stacked() {
        let masks = halves(4, 4, |x, _| if x < 2 { [0, 255, 0, 255] } else { [255, 0, 0, 255] });
        let grey = halves(2, 2, |_, _| [128, 128, 128, 255]);
        let item = GunSkin::new(&params("Common_Materials.Items.Master_ShieldGrenade", "p_ColorD"), masks.clone(), grey.clone(), SkinDetail::Flat);
        let gun = GunSkin::new(&params("Common_Materials.Weapons.Master_Gun", "p_DColor"), masks, grey, SkinDetail::Flat);
        assert_eq!(strongest(item.shade(&sample(0.25, 0.5))), 0, "item: zone A, red");
        assert_eq!(strongest(gun.shade(&sample(0.25, 0.5))), 2, "gun: zone B, blue");
    }

    /// Where no zone is marked the fourth colour shows: `p_ColorD` on items, not white.
    #[test]
    fn items_take_their_fourth_colour_from_p_color_d() {
        let blank = halves(4, 4, |_, _| [0, 0, 0, 255]);
        let grey = halves(2, 2, |_, _| [128, 128, 128, 255]);
        let item = GunSkin::new(&params("Common_Materials.Items.Master_ShieldGrenade", "p_ColorD"), blank, grey, SkinDetail::Flat);
        let c = item.shade(&sample(0.3, 0.3));
        assert!(c[1] > 100 && c[0] < 20 && c[2] < 20, "{c:?}");
    }

    #[test]
    fn item_patterns_without_a_channel_mask_are_read_as_grey() {
        let mut p = params("Common_Materials.Items.Master_ShieldGrenade", "p_ColorD");
        p.vectors.insert("p_patternchannelscale".into(), [1.0, 0.0, 0.0, 0.0]);
        let pattern = halves(2, 2, |_, _| [200, 0, 255, 255]);
        let masks = halves(4, 4, |x, _| if x < 2 { [0, 0, 0, 255] } else { [255, 0, 0, 255] });
        let grey = halves(2, 2, |_, _| [128, 128, 128, 255]);
        // Zone A in white so the pattern's own colour would show if it were read as colour.
        for tone in ["hilight", "midtone", "shadow"] {
            p.vectors.insert(format!("p_acolor{tone}"), [1.0, 1.0, 1.0, 1.0]);
        }
        let skin = GunSkin::new(&p, masks, grey, SkinDetail::Flat).with_pattern(&p, pattern);
        let c = skin.shade(&sample(0.25, 0.5));
        // Grey: the red channel (200) as a grey mask, so no magenta cast.
        assert!(c[0].abs_diff(c[1]) < 3 && c[1].abs_diff(c[2]) < 3, "{c:?}");
    }

    #[test]
    fn relics_glow_in_their_colour() {
        let green = RelicSkin::new([0.0, 1.0, 0.1], None);
        assert_eq!(strongest(green.shade(&sample(0.5, 0.5))), 1);
        // A black glow would draw a black relic: the material's default blue instead.
        let none = RelicSkin::new([0.0, 0.0, 0.0], None);
        assert_eq!(strongest(none.shade(&sample(0.5, 0.5))), 2);
        // The glyphs (blue channel of the texture) are brighter than the bare surface.
        let texture = halves(2, 2, |x, _| if x == 0 { [128, 0, 0, 255] } else { [128, 0, 255, 255] });
        let skin = RelicSkin::new([1.0, 0.2, 0.0], Some(texture));
        let (bare, glyph) = (skin.shade(&sample(0.25, 0.5)), skin.shade(&sample(0.75, 0.5)));
        assert!(glyph.iter().map(|&c| u32::from(c)).sum::<u32>() > bare.iter().map(|&c| u32::from(c)).sum::<u32>());
    }
}
