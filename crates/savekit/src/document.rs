use crate::codec;
use crate::error::{Result, SaveKitError};
use crate::field;
use crate::proto::ProtoMessage;

/// Item struct version used by Borderlands: The Pre-Sequel (Borderlands 2 uses 7).
const PRE_SEQUEL_ITEM_VERSION: u8 = 10;

/// A save file as an order-preserving protobuf message.
///
/// Character values and edits are in `character.rs`, items in `items/inventory.rs`.
/// Every edit is all-or-nothing: if it returns an error, the document is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveDocument {
    pub message: ProtoMessage,
}

impl SaveDocument {
    pub fn new(message: ProtoMessage) -> Self {
        SaveDocument { message }
    }

    /// Decodes the bytes of a `.sav` file. Rejects Pre-Sequel saves.
    pub fn from_file_bytes(file: &[u8]) -> Result<Self> {
        let message = ProtoMessage::decode(&codec::decode(file)?)?;
        // Format doc 6.2: the first byte of every item or weapon blob is not obfuscated and
        // holds the item struct version in its low 7 bits.
        let item_fields = [field::BANK, field::ITEMS, field::WEAPONS];
        for entry in message.fields.iter().filter(|f| item_fields.contains(&f.number)) {
            let header = entry
                .value
                .as_bytes()
                .and_then(|bytes| ProtoMessage::decode(bytes).ok())
                .and_then(|sub| sub.bytes(1).and_then(|blob| blob.first().copied()));
            if header.is_some_and(|h| h & 0x7F == PRE_SEQUEL_ITEM_VERSION) {
                return Err(SaveKitError::NotASave(
                    "Borderlands: The Pre-Sequel saves are not supported".to_string(),
                ));
            }
        }
        Ok(SaveDocument { message })
    }

    /// Encodes the document as the bytes of a `.sav` file. Fails only if the save is too
    /// large to be read back (see [`codec::MAX_DECLARED_LEN`]).
    pub fn to_file_bytes(&self) -> Result<Vec<u8>> {
        codec::encode(&self.message.encode())
    }

    /// Runs `body` on a copy of the message and keeps the copy only if `body` succeeds.
    pub(crate) fn edit(&mut self, body: impl FnOnce(&mut ProtoMessage) -> Result<()>) -> Result<()> {
        let mut copy = self.message.clone();
        body(&mut copy)?;
        self.message = copy;
        Ok(())
    }
}
