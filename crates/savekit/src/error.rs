use std::fmt;

/// Everything that can go wrong while reading, editing or writing a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveKitError {
    /// The bytes are not a Borderlands 2 PC save (too short, no `WSG` magic, absurd lengths,
    /// a Pre-Sequel save).
    NotASave(String),
    /// The SHA-1 at the start of the file does not match the rest of it.
    HashMismatch,
    /// A console (big-endian) save.
    UnsupportedByteOrder,
    /// The WSG version field is not 2.
    UnsupportedVersion(u32),
    /// The decoded payload does not match the CRC-32 in the WSG header.
    CrcMismatch,
    CorruptCompression(String),
    CorruptHuffman(String),
    CorruptProtobuf(String),
    /// An item blob is too short to hold a checksum, or its checksum is wrong.
    ItemChecksumMismatch,
    /// A value passed to an edit is out of range, or a field has an encoding the edit refuses
    /// to touch. Also used by the verifier, with `field == "verify"`.
    InvalidValue { field: String, reason: String },
    BackpackFull,
    NotDuplicable(String),
}

pub type Result<T> = std::result::Result<T, SaveKitError>;

impl SaveKitError {
    pub(crate) fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Self {
        SaveKitError::InvalidValue { field: field.into(), reason: reason.into() }
    }
}

impl fmt::Display for SaveKitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotASave(why) => write!(f, "not a Borderlands 2 save: {why}"),
            Self::HashMismatch => f.write_str("the file's SHA-1 hash does not match its contents"),
            Self::UnsupportedByteOrder => f.write_str("console (big-endian) saves are not supported"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported save version {v}"),
            Self::CrcMismatch => f.write_str("the save's CRC-32 does not match its contents"),
            Self::CorruptCompression(why) => write!(f, "corrupt compressed data: {why}"),
            Self::CorruptHuffman(why) => write!(f, "corrupt Huffman data: {why}"),
            Self::CorruptProtobuf(why) => write!(f, "corrupt save data: {why}"),
            Self::ItemChecksumMismatch => f.write_str("item data failed its checksum"),
            Self::InvalidValue { field, reason } => write!(f, "{field}: {reason}"),
            Self::BackpackFull => f.write_str("the backpack is full"),
            Self::NotDuplicable(why) => write!(f, "cannot duplicate: {why}"),
        }
    }
}

impl std::error::Error for SaveKitError {}

impl From<lzo1x::LzoError> for SaveKitError {
    fn from(e: lzo1x::LzoError) -> Self {
        SaveKitError::CorruptCompression(e.0.to_string())
    }
}
