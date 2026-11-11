//! A schema-less protobuf message that keeps its fields in their original order, so an
//! unedited save is written back byte for byte (format doc section 4.1).

use crate::error::{Result, SaveKitError};

fn fail(why: impl Into<String>) -> SaveKitError {
    SaveKitError::CorruptProtobuf(why.into())
}

/// Largest field number the decoder accepts.
pub const MAX_FIELD_NUMBER: u32 = i32::MAX as u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoValue {
    /// Wire type 0. Negative numbers are stored as 64-bit two's complement.
    Varint(u64),
    /// Wire type 1.
    Fixed64(u64),
    /// Wire type 2: a string, a nested message, a packed array or a blob. Kept raw.
    Bytes(Vec<u8>),
    /// Wire type 5.
    Fixed32(u32),
}

impl ProtoValue {
    pub fn as_varint(&self) -> Option<u64> {
        match self {
            ProtoValue::Varint(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ProtoValue::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoField {
    pub number: u32,
    pub value: ProtoValue,
}

impl ProtoField {
    pub fn new(number: u32, value: ProtoValue) -> Self {
        ProtoField { number, value }
    }

    pub fn varint(number: u32, value: u64) -> Self {
        ProtoField { number, value: ProtoValue::Varint(value) }
    }

    pub fn bytes(number: u32, value: impl Into<Vec<u8>>) -> Self {
        ProtoField { number, value: ProtoValue::Bytes(value.into()) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtoMessage {
    pub fields: Vec<ProtoField>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    /// Fails on truncation, on more than 10 bytes, and on a value that overflows 64 bits.
    fn varint(&mut self) -> Result<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.bytes.get(self.pos).ok_or_else(|| fail("truncated varint"))?;
            self.pos += 1;
            if shift == 63 && byte > 1 {
                return Err(fail("varint overflows 64 bits"));
            }
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift > 63 {
                return Err(fail("varint too long"));
            }
        }
    }

    fn take(&mut self, count: usize, what: &str) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(count).filter(|&end| end <= self.bytes.len());
        let end = end.ok_or_else(|| fail(format!("truncated {what}")))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value & 0x7F) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

impl ProtoMessage {
    pub fn new(fields: Vec<ProtoField>) -> Self {
        ProtoMessage { fields }
    }

    /// Parses wire types 0, 1, 2 and 5. Groups (3, 4) and anything else are rejected.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader { bytes, pos: 0 };
        let mut fields = Vec::new();
        while !reader.at_end() {
            let key = reader.varint()?;
            let number = key >> 3;
            if number < 1 || number > u64::from(MAX_FIELD_NUMBER) {
                return Err(fail(format!("bad field number {number}")));
            }
            let value = match key & 7 {
                0 => ProtoValue::Varint(reader.varint()?),
                1 => {
                    let raw = reader.take(8, "fixed64")?;
                    ProtoValue::Fixed64(raw.iter().rev().fold(0, |v, &b| (v << 8) | u64::from(b)))
                }
                2 => {
                    let len = reader.varint()?;
                    let len = usize::try_from(len).map_err(|_| fail("length runs past end"))?;
                    ProtoValue::Bytes(reader.take(len, "length-delimited field")?.to_vec())
                }
                5 => {
                    let raw = reader.take(4, "fixed32")?;
                    ProtoValue::Fixed32(raw.iter().rev().fold(0, |v, &b| (v << 8) | u32::from(b)))
                }
                wire => return Err(fail(format!("unsupported wire type {wire}"))),
            };
            fields.push(ProtoField { number: number as u32, value });
        }
        Ok(ProtoMessage { fields })
    }

    /// Serialises the fields in the order they are held, with minimal varints.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for field in &self.fields {
            let key = u64::from(field.number) << 3;
            match &field.value {
                ProtoValue::Varint(v) => {
                    write_varint(key, &mut out);
                    write_varint(*v, &mut out);
                }
                ProtoValue::Fixed64(v) => {
                    write_varint(key | 1, &mut out);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                ProtoValue::Bytes(b) => {
                    write_varint(key | 2, &mut out);
                    write_varint(b.len() as u64, &mut out);
                    out.extend_from_slice(b);
                }
                ProtoValue::Fixed32(v) => {
                    write_varint(key | 5, &mut out);
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }

    /// Every value of field `number`, in message order.
    pub fn values(&self, number: u32) -> impl Iterator<Item = &ProtoValue> {
        self.fields.iter().filter(move |f| f.number == number).map(|f| &f.value)
    }

    /// How many entries field `number` has.
    pub fn count(&self, number: u32) -> usize {
        self.values(number).count()
    }

    /// The first value of field `number`.
    pub fn first(&self, number: u32) -> Option<&ProtoValue> {
        self.values(number).next()
    }

    /// The first value of field `number`, if it is a varint.
    pub fn varint(&self, number: u32) -> Option<u64> {
        self.first(number)?.as_varint()
    }

    /// The first value of field `number`, if it is length-delimited.
    pub fn bytes(&self, number: u32) -> Option<&[u8]> {
        self.first(number)?.as_bytes()
    }

    /// Replaces every entry of `number` with one entry at the first existing position. If
    /// there is none, inserts before the first field with a higher number, or at the end.
    pub fn set_single(&mut self, number: u32, value: ProtoValue) {
        match self.fields.iter().position(|f| f.number == number) {
            Some(first) => {
                self.fields[first].value = value;
                let mut index = 0;
                self.fields.retain(|f| {
                    let keep = f.number != number || index == first;
                    index += 1;
                    keep
                });
            }
            None => self.insert_in_order(ProtoField { number, value }),
        }
    }

    /// Adds one more entry of a repeated field: after the last entry with the same number, or
    /// else before the first field with a higher number, or at the end. Returns the new entry's
    /// position among the entries of `number` (0-based).
    pub fn push_repeated(&mut self, number: u32, value: ProtoValue) -> usize {
        let existing = self.count(number);
        let field = ProtoField { number, value };
        match self.fields.iter().rposition(|f| f.number == number) {
            Some(last) => self.fields.insert(last + 1, field),
            None => self.insert_in_order(field),
        }
        existing
    }

    fn insert_in_order(&mut self, field: ProtoField) {
        match self.fields.iter().position(|f| f.number > field.number) {
            Some(at) => self.fields.insert(at, field),
            None => self.fields.push(field),
        }
    }
}

/// Decodes a packed repeated varint field (the currency and black-market arrays).
pub fn decode_packed_varints(bytes: &[u8]) -> Result<Vec<u64>> {
    let mut reader = Reader { bytes, pos: 0 };
    let mut out = Vec::new();
    while !reader.at_end() {
        out.push(reader.varint()?);
    }
    Ok(out)
}

pub fn encode_packed_varints(values: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in values {
        write_varint(v, &mut out);
    }
    out
}
