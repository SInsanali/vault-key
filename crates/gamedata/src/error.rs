use std::fmt;

/// Everything that can go wrong while reading game data. Parsing never panics on file
/// contents; it returns one of these instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameDataError {
    /// No usable install (or one of its files) at this path.
    InstallNotFound(String),
    /// The bytes do not start with the UE3 package tag.
    NotAPackage(String),
    /// A package from another engine build. Only version 832, licensee 46 is understood.
    UnsupportedPackage { version: u16, licensee: u16 },
    /// A read ran past the end of the data.
    Truncated(String),
    /// An LZO block inside a package did not decompress.
    CorruptCompression(String),
    /// A name, import or export reference points outside its table.
    BadReference(String),
    /// Well-formed bytes with a shape this reader does not expect.
    UnexpectedData(String),
    /// The cache directory is inside the game install, which is never written to.
    CacheLocationInsideGame,
    /// The cache directory or file could not be created, read or written.
    CacheUnreadable(String),
    /// The caller canceled the import.
    Canceled,
}

impl fmt::Display for GameDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InstallNotFound(s) => write!(f, "game install not found: {s}"),
            Self::NotAPackage(s) => write!(f, "not a game package: {s}"),
            Self::UnsupportedPackage { version, licensee } => {
                write!(
                    f,
                    "unsupported package version {version}, licensee {licensee}"
                )
            }
            Self::Truncated(s) => write!(f, "truncated data: {s}"),
            Self::CorruptCompression(s) => write!(f, "corrupt compression: {s}"),
            Self::BadReference(s) => write!(f, "bad reference: {s}"),
            Self::UnexpectedData(s) => write!(f, "unexpected data: {s}"),
            Self::CacheLocationInsideGame => {
                write!(f, "the cache location is inside the game install")
            }
            Self::CacheUnreadable(s) => write!(f, "cache unreadable: {s}"),
            Self::Canceled => write!(f, "canceled"),
        }
    }
}

impl std::error::Error for GameDataError {}

pub(crate) type Result<T> = std::result::Result<T, GameDataError>;
