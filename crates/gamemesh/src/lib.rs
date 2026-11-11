//! Weapon pictures from the user's own game install.
//!
//! Reads the game's "gestalt" weapon meshes out of `Startup.upk` (read-only), picks the pieces
//! a saved weapon's parts name, colours them from the weapon's own skin, and draws them with a
//! small software renderer.
//!
//! - [`Pictures`]: the entry point for the app. An item code in, a PNG out.
//! - [`WeaponSource`]: from a saved weapon's numbers to an [`Assembly`] of mesh fragments.
//! - [`SkeletalMesh`], [`GestaltMesh`], [`texture`], [`material`]: the file formats.
//! - [`render`], [`png`], [`deflate`]: triangles to pixels to a compressed PNG file.
//!
//! Nothing here writes anywhere, and the game install is only read. Parsing never panics on
//! file contents: every read is bounds-checked and failures come back as errors.
//!
//! Research notes: `docs/research/2026-11-09-weapon-pictures-spike.md`.

#![forbid(unsafe_code)]

pub mod balances;
pub mod deflate;
mod error;
mod gestalt;
pub mod material;
pub mod picture;
mod pictures;
pub mod png;
mod reader;
pub mod render;
mod skeletal;
pub mod texture;
mod weapon;

pub use error::MeshError;
pub use gestalt::{Fragment, GestaltMesh};
pub use pictures::{reduce_depth, Colouring, Facing, PictureError, Pictures, Style};
pub use skeletal::{Bone, Chunk, Section, SkeletalMesh, Vertex};
pub use weapon::{fragment_names, Assembly, AssemblyPart, PartLook, WeaponSource, MATERIAL_SLOT, WEAPON_SLOTS};
