use std::fmt;

use gamedata::GameDataError;

/// What can go wrong while reading a mesh or assembling a weapon. Parsing never panics on
/// file contents.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshError {
    /// The package layer failed (missing file, bad package, bad reference).
    Package(GameDataError),
    /// A read ran past the end of the object's data.
    Truncated(String),
    /// Well-formed bytes with a shape this reader does not handle.
    Unsupported(String),
    /// A definition the item refers to is not in the package, or lacks a needed property.
    Missing(String),
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(e) => write!(f, "{e}"),
            Self::Truncated(s) => write!(f, "truncated mesh data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported mesh data: {s}"),
            Self::Missing(s) => write!(f, "missing: {s}"),
        }
    }
}

impl std::error::Error for MeshError {}

impl From<GameDataError> for MeshError {
    fn from(e: GameDataError) -> Self {
        Self::Package(e)
    }
}

pub type Result<T> = std::result::Result<T, MeshError>;
