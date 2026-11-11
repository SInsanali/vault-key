//! Builds the item database from decompressed packages.

mod builder;
mod object_index;
pub(crate) mod stats;

pub use builder::build_database;
