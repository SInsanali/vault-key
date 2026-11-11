//! The entry point for the app: an item code in, a PNG picture out.

use std::path::Path;
use std::sync::Arc;

use gamedata::{GameInstall, ItemRefs};
use savekit::{ItemBlob, ItemFields};

use crate::error::MeshError;
use crate::material::{GunSkin, SkinDetail};
use crate::picture::weapon_scene;
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
    /// A weapon from a DLC pack: not supported yet.
    DlcItem,
    /// The weapon's type or some of its parts are not defined in the game files read.
    UnknownParts,
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
            Self::DlcItem => "Pictures of DLC items are not supported yet.",
            Self::UnknownParts => "The game files do not define some of this item's parts.",
            Self::GameData(_) => "A game file is missing or not as expected, so the picture could not be made.",
        }
    }
}

impl std::fmt::Display for PictureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GameData(detail) => write!(f, "{} ({detail})", self.message()),
            _ => f.write_str(self.message()),
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

/// How the weapon is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Colouring {
    /// Neutral colours per part slot; needs no textures.
    Plain,
    /// The skin's colour zones as flat colours.
    Flat,
    /// The skin's colour zones with the game's drawn lines and highlights.
    #[default]
    Textured,
}

/// How pictures are drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub facing: Facing,
    pub colouring: Colouring,
    /// Turn of the camera around the vertical axis, and its rise, in degrees. 0, 0 is a flat
    /// side view.
    pub yaw: f32,
    pub pitch: f32,
    /// Outline colour.
    pub ink: [u8; 3],
    /// A light rim outside the silhouette, for dark pages. `None` for no rim.
    pub rim: Option<[u8; 3]>,
    /// Bits kept per colour channel in the PNG (4 to 8). Fewer bits make a smaller file; the
    /// texture grain hides the steps. Alpha always keeps 8.
    pub colour_bits: u8,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            facing: Facing::default(),
            colouring: Colouring::default(),
            yaw: 0.0,
            pitch: 0.0,
            ink: [20, 17, 14],
            rim: Some([236, 222, 196]),
            colour_bits: 6,
        }
    }
}

/// Keeps the top `bits` bits of each colour byte and repeats them below, so that black
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
        let pixels = Arc::new(texture.decode(mip, source.package_dir())?);
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

/// Weapon pictures from the user's own game install. Opening reads `Startup.upk` once (the
/// package is not kept); pictures then read only the texture cache file, and only for
/// textures not already in memory.
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

    /// Opens the game's startup package (read-only).
    pub fn open(install: &GameInstall) -> Result<Self, PictureError> {
        Self::open_package(&install.startup_package())
    }

    /// As [`Pictures::open`], from the path of `Startup.upk`.
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

    /// Why the last picture was drawn in plain colours instead of its skin, if it was. A
    /// missing or damaged texture does not fail the picture.
    pub fn skin_problem(&self) -> Option<&str> {
        self.skin_problem.as_deref()
    }

    /// A PNG of the weapon an item code describes, `width` × `height` pixels, on a
    /// transparent background.
    pub fn weapon_png(&mut self, code: &str, width: u32, height: u32) -> Result<Vec<u8>, PictureError> {
        let mut image = self.weapon_image(&Self::decode(code)?, width, height)?;
        reduce_depth(&mut image.rgba, self.style.colour_bits);
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
            Some(0) => {}
            Some(255) => return Err(PictureError::NothingToDraw),
            Some(_) => return Err(PictureError::DlcItem),
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
        let (scene, missing) = weapon_scene(gestalt, assembly);
        if let Some(name) = missing.first() {
            return Err(PictureError::GameData(format!("{} has no fragment {name}", assembly.gestalt)));
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
            // Items are modelled facing the other way: drawn like a weapon, their lettering is
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
        let detail = match self.style.colouring {
            Colouring::Plain => None,
            Colouring::Flat => Some(SkinDetail::Flat),
            Colouring::Textured => Some(SkinDetail::Textured),
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
                if let Ok(pixels) = self.textures.get(&self.source, path, Self::LAYER_TEXTURE_SIZE) {
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
