//! The entry point for the app: an item code in, a PNG picture out.

use std::path::Path;
use std::sync::Arc;

use gamedata::{GameInstall, ItemRefs};
use savekit::{ItemBlob, ItemFields};

use crate::error::MeshError;
use crate::material::{GunSkin, RelicSkin, SkinDetail, RELIC_TEXTURE};
use crate::picture::{face_camera, weapon_scene};
use crate::png;
use crate::render::{render, render_with, Image, View};
use crate::texture::Pixels;
use crate::weapon::{WeaponSource, MATERIAL_SLOT};

/// Why there is no picture. Each has a short sentence for the user in [`PictureError::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictureError {
    /// The text is not a readable item code.
    BadCode,
    /// A kind of item with no model to draw: a story or mission item.
    NotAWeapon,
    /// A placeholder entry, or a weapon none of whose parts has a model.
    NothingToDraw,
    /// The weapon's type or some of its parts are not defined in the game files read.
    UnknownParts,
    /// A hybrid: some part is defined, but made for another kind of item, so the item's model
    /// has no piece for it. The text names the piece. The game files are fine.
    Hybrid(String),
    /// A game file is missing, damaged or not laid out as expected. The text says which.
    GameData(String),
}

impl PictureError {
    /// A short sentence for the user.
    pub fn message(&self) -> &'static str {
        match self {
            Self::BadCode => "This is not a readable item code.",
            Self::NotAWeapon => "Story and mission items have no picture.",
            Self::NothingToDraw => "This item has no model to draw.",
            Self::UnknownParts => "The game files do not define some of this item's parts.",
            Self::Hybrid(_) => "Some of this item's parts are made for another kind of item, so it has no picture.",
            Self::GameData(_) => "A game file is missing or not as expected, so the picture could not be made.",
        }
    }

    /// The detail behind the sentence, for a log: which piece or which file. `None` when the
    /// sentence says it all.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Hybrid(detail) | Self::GameData(detail) => Some(detail),
            _ => None,
        }
    }
}

impl std::fmt::Display for PictureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.detail() {
            Some(detail) => write!(f, "{} ({detail})", self.message()),
            None => f.write_str(self.message()),
        }
    }
}

impl std::error::Error for PictureError {}

/// Which way the weapon points. Neither is a mirror image: each shows a different side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Facing {
    /// Muzzle to the left: the weapon's left side, as the game's own item tiles draw it.
    #[default]
    MuzzleLeft,
    /// Muzzle to the right: the weapon's right side (the ejection-port side).
    MuzzleRight,
}

/// How the weapon is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coloring {
    /// Neutral colors per part slot; needs no textures.
    Plain,
    /// The skin's color zones as flat colors.
    Flat,
    /// The skin's color zones with the game's drawn lines and highlights.
    #[default]
    Textured,
}

/// How pictures are drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub facing: Facing,
    pub coloring: Coloring,
    /// Turn of the camera around the vertical axis, and its rise, in degrees. 0, 0 is a flat
    /// side view.
    pub yaw: f32,
    pub pitch: f32,
    /// Outline color.
    pub ink: [u8; 3],
    /// A light rim outside the silhouette, for dark pages. `None` for no rim.
    pub rim: Option<[u8; 3]>,
    /// Bits kept per color channel in the PNG (4 to 8). Fewer bits make a smaller file; the
    /// texture grain hides the steps. Alpha always keeps 8.
    pub color_bits: u8,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            facing: Facing::default(),
            coloring: Coloring::default(),
            yaw: 0.0,
            pitch: 0.0,
            ink: [20, 17, 14],
            rim: Some([236, 222, 196]),
            color_bits: 6,
        }
    }
}

/// Keeps the top `bits` bits of each color byte and repeats them below, so that black
/// stays black and white stays white. Alpha is left alone.
pub fn reduce_depth(rgba: &mut [u8], bits: u8) {
    let bits = u32::from(bits.clamp(4, 8));
    if bits == 8 {
        return;
    }
    for pixel in rgba.as_chunks_mut::<4>().0 {
        for c in &mut pixel[..3] {
            let top = *c >> (8 - bits);
            *c = (top << (8 - bits)) | (top >> (2 * bits - 8));
        }
    }
}

/// Decoded textures, least recently used dropped first once over budget.
struct TextureCache {
    entries: Vec<(String, u32, Arc<Pixels>, u64)>,
    clock: u64,
}

impl TextureCache {
    fn get(&mut self, source: &WeaponSource, path: &str, size: u32) -> Result<Arc<Pixels>, MeshError> {
        self.clock += 1;
        if let Some(entry) = self.entries.iter_mut().find(|e| e.0 == path && e.1 == size) {
            entry.3 = self.clock;
            return Ok(entry.2.clone());
        }
        let texture = source.texture(path).ok_or_else(|| MeshError::Missing(format!("texture {path}")))?;
        let mip = texture.mip_for(size).ok_or_else(|| MeshError::Missing(format!("{path}: no mips")))?;
        let pixels = Arc::new(texture.decode_from(mip, &source.texture_dirs(path))?);
        self.entries.push((path.to_string(), size, pixels.clone(), self.clock));
        // Over budget: drop the least recently used, but never what was just loaded.
        while self.entries.len() > 2 && self.bytes() > Pictures::TEXTURE_BUDGET {
            let oldest = (0..self.entries.len()).min_by_key(|&i| self.entries[i].3).unwrap_or(0);
            self.entries.swap_remove(oldest);
        }
        Ok(pixels)
    }

    fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.2.byte_len()).sum()
    }
}

/// Weapon and item pictures from the user's own game install. Opening reads the base game's
/// `Startup.upk` and each DLC's startup package once (the packages are not kept); pictures then
/// read only the texture cache files, and only for textures not already in memory.
///
/// Memory kept between calls: the weapon meshes and tables (about 10 MB) and at most
/// [`Pictures::TEXTURE_BUDGET`] of decoded textures.
pub struct Pictures {
    source: WeaponSource,
    textures: TextureCache,
    style: Style,
    skin_problem: Option<String>,
}

impl Pictures {
    /// Upper bound on decoded textures kept in memory. Two textures per weapon class at up to
    /// 16 MiB each for large pictures, 4 MiB each for small ones.
    pub const TEXTURE_BUDGET: usize = 64 << 20;
    /// Largest picture drawn, in pixels; larger requests are scaled down, keeping their shape.
    pub const MAX_PIXELS: u64 = 2_600_000;
    /// Pictures up to this many pixels wide use the smaller textures.
    const SMALL_WIDTH: u32 = 480;
    /// Pattern and decal textures are read at up to this size (most are 256 px).
    const LAYER_TEXTURE_SIZE: u32 = 256;

    /// Opens the game's startup packages, the base game's and each DLC's (read-only).
    pub fn open(install: &GameInstall) -> Result<Self, PictureError> {
        let source = WeaponSource::open_install(install).map_err(|e| PictureError::GameData(e.to_string()))?;
        Ok(Self::new(source))
    }

    /// From the base game's `Startup.upk` alone: no DLC items.
    pub fn open_package(startup_package: &Path) -> Result<Self, PictureError> {
        let source = WeaponSource::open(startup_package).map_err(|e| PictureError::GameData(e.to_string()))?;
        Ok(Self::new(source))
    }

    pub fn new(source: WeaponSource) -> Self {
        Self { source, textures: TextureCache { entries: Vec::new(), clock: 0 }, style: Style::default(), skin_problem: None }
    }

    pub fn source(&self) -> &WeaponSource {
        &self.source
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    /// Bytes of decoded texture held right now.
    pub fn texture_bytes(&self) -> usize {
        self.textures.bytes()
    }

    /// Why the last picture was drawn in plain colors instead of its skin, if it was. A
    /// missing or damaged texture does not fail the picture.
    pub fn skin_problem(&self) -> Option<&str> {
        self.skin_problem.as_deref()
    }

    /// A PNG of the weapon an item code describes, `width` × `height` pixels, on a
    /// transparent background.
    pub fn weapon_png(&mut self, code: &str, width: u32, height: u32) -> Result<Vec<u8>, PictureError> {
        let mut image = self.weapon_image(&Self::decode(code)?, width, height)?;
        reduce_depth(&mut image.rgba, self.style.color_bits);
        Ok(png::encode_rgba(image.width, image.height, &image.rgba))
    }

    /// The numbers in an item code.
    pub fn decode(code: &str) -> Result<ItemRefs, PictureError> {
        let blob = ItemBlob::from_code(code.trim()).map_err(|_| PictureError::BadCode)?;
        let fields = ItemFields::from_blob(&blob);
        Ok(ItemRefs {
            is_weapon: blob.is_weapon,
            set: fields.set,
            item_type: fields.item_type,
            balance: fields.balance,
            manufacturer: fields.manufacturer,
            parts: fields.parts,
        })
    }

    /// The picture as pixels (straight alpha).
    pub fn weapon_image(&mut self, item: &ItemRefs, width: u32, height: u32) -> Result<Image, PictureError> {
        match item.set {
            None => return Err(PictureError::BadCode),
            Some(255) => return Err(PictureError::NothingToDraw),
            Some(_) => {}
        }
        let assembly = self.source.assemble(item).map_err(|e| match e {
            MeshError::Missing(_) => PictureError::UnknownParts,
            MeshError::Unsupported(_) if !item.is_weapon => PictureError::NotAWeapon,
            other => PictureError::GameData(other.to_string()),
        })?;
        if assembly.unresolved_slots.iter().any(|&slot| slot != MATERIAL_SLOT) {
            return Err(PictureError::UnknownParts);
        }
        self.assembly_image(&assembly, width, height)
    }

    /// The picture of an already assembled weapon.
    pub fn assembly_image(&mut self, assembly: &crate::Assembly, width: u32, height: u32) -> Result<Image, PictureError> {
        let game_data = |e: MeshError| PictureError::GameData(e.to_string());
        let gestalt = self.source.gestalt(&assembly.gestalt).map_err(game_data)?;
        let (mut scene, missing) = weapon_scene(gestalt, assembly).map_err(game_data)?;
        if assembly.fragments().map(|(_, name)| name).eq([crate::weapon::WHOLE_MESH]) {
            face_camera(&mut scene);
        }
        // A piece this model lacks but another model has: a part made for another kind of
        // item (a hybrid). A piece no model has: the game files are not as expected.
        if let Some(name) = missing.first() {
            let detail = format!("{} has no fragment {name}", assembly.gestalt);
            let elsewhere = self
                .source
                .gestalt_paths()
                .filter_map(|path| self.source.gestalt(path).ok())
                .any(|other| other.fragments_named(name).next().is_some());
            return Err(if elsewhere { PictureError::Hybrid(detail) } else { PictureError::GameData(detail) });
        }
        if scene.triangles.is_empty() {
            return Err(PictureError::NothingToDraw);
        }

        // Size: within the pixel budget, with more samples per pixel for small pictures.
        let (mut w, mut h) = (u64::from(width.max(1)), u64::from(height.max(1)));
        if w * h > Self::MAX_PIXELS {
            let shrink = (Self::MAX_PIXELS as f64 / (w * h) as f64).sqrt();
            (w, h) = (((w as f64 * shrink) as u64).max(1), ((h as f64 * shrink) as u64).max(1));
        }
        let supersample = match w * h {
            0..=290_000 => 3,
            290_001..=650_000 => 2,
            _ => 1,
        };
        let line_width = (w as f32 / 520.0).clamp(0.8, 2.6);
        let view = View {
            width: w as u32,
            height: h as u32,
            yaw: self.style.yaw,
            pitch: self.style.pitch,
            // Items are modeled facing the other way: drawn like a weapon, their lettering is
            // mirrored.
            muzzle_right: (self.style.facing == Facing::MuzzleRight) != assembly.is_item,
            supersample,
            background: None,
            ink: self.style.ink,
            line_width,
            rim: self.style.rim,
            ..View::default()
        };

        self.skin_problem = None;
        // A relic has no material part: it glows in the color its body part sets.
        if let (None, Some(glow), false) = (&assembly.material, assembly.glow, self.style.coloring == Coloring::Plain) {
            let texture = self.textures.get(&self.source, RELIC_TEXTURE, Self::LAYER_TEXTURE_SIZE);
            if let Err(e) = &texture {
                self.skin_problem = Some(e.to_string());
            }
            return Ok(render_with(&scene, &view, &RelicSkin::new(glow, texture.ok())));
        }
        let detail = match self.style.coloring {
            Coloring::Plain => None,
            Coloring::Flat => Some(SkinDetail::Flat),
            Coloring::Textured => Some(SkinDetail::Textured),
        };
        let skin = detail.and_then(|detail| match self.skin(assembly, w as u32, detail) {
            Ok(skin) => Some(skin),
            Err(problem) => {
                self.skin_problem = Some(problem);
                None
            }
        });
        Ok(match skin {
            Some(skin) => render_with(&scene, &view, &skin),
            None => render(&scene, &view),
        })
    }

    /// The mip to read a pattern at. A pattern is tiled many times across the item (12 to 30
    /// times on shields and grenades); read at full size, each pixel of a small picture lands
    /// on an unrelated texel and the pattern turns into glitter. A mip with about one texel per
    /// pixel per tile averages it the way the game's own texture filtering does.
    fn pattern_size(params: &crate::material::MaterialParameters, width: u32) -> u32 {
        let scale = params.vector("p_PatternScalePosition").map_or(1.0, |s| s[0].abs().max(s[1].abs()));
        let scale = if scale.is_finite() { scale.max(1.0) } else { 1.0 };
        let per_tile = (width as f32 / scale).ceil().clamp(4.0, Self::LAYER_TEXTURE_SIZE as f32) as u32;
        per_tile.next_power_of_two().min(Self::LAYER_TEXTURE_SIZE)
    }

    fn skin(&mut self, assembly: &crate::Assembly, width: u32, detail: SkinDetail) -> Result<GunSkin, String> {
        let material = assembly.material.as_deref().ok_or("the weapon has no material part")?;
        let params = self.source.material(material).ok_or_else(|| format!("material {material} was not read"))?;
        let (masks, diffuse) = GunSkin::texture_paths(params).ok_or_else(|| format!("material {material} names no textures"))?;
        let size = if width <= Self::SMALL_WIDTH { 1024 } else { 2048 };
        let masks = self.textures.get(&self.source, masks, size).map_err(|e| e.to_string())?;
        let diffuse = self.textures.get(&self.source, diffuse, size).map_err(|e| e.to_string())?;
        let mut skin = GunSkin::new(params, masks, diffuse, detail);
        if detail == SkinDetail::Textured {
            // Patterns and decals are small textures; a missing one leaves the skin plain.
            if let Some(path) = GunSkin::pattern_path(params) {
                if let Ok(pixels) = self.textures.get(&self.source, path, Self::pattern_size(params, width)) {
                    skin = skin.with_pattern(params, pixels);
                }
            }
            if let Some(path) = GunSkin::decal_path(params) {
                if let Ok(pixels) = self.textures.get(&self.source, path, Self::LAYER_TEXTURE_SIZE) {
                    skin = skin.with_decal(params, pixels);
                }
            }
        }
        Ok(skin)
    }
}
