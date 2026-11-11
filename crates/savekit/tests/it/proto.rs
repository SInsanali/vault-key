use savekit::proto::{decode_packed_varints, encode_packed_varints, MAX_FIELD_NUMBER};
use savekit::{codec, ProtoField, ProtoMessage, ProtoValue, SaveDocument, SaveKitError};

use crate::support::sample_files;

fn numbers(m: &ProtoMessage) -> Vec<u32> {
    m.fields.iter().map(|f| f.number).collect()
}

/// An unedited decode, encode, decode gives the same payload, byte for byte.
#[test]
fn unmodified_round_trip_is_identical() {
    for (name, file) in sample_files() {
        let payload = codec::decode(&file).unwrap();
        assert_eq!(ProtoMessage::decode(&payload).unwrap().encode(), payload, "{name}: message");
        let again = SaveDocument::from_file_bytes(&file).unwrap().to_file_bytes().unwrap();
        assert_eq!(codec::decode(&again).unwrap(), payload, "{name}: file");
    }
}

#[test]
fn varint_edges() {
    let values = [0, 1, 127, 128, 300, u64::from(u32::MAX), 1 << 63, u64::MAX, -3i64 as u64];
    let message = ProtoMessage::new(values.iter().map(|&v| ProtoField::varint(2, v)).collect());
    let encoded = message.encode();
    assert_eq!(ProtoMessage::decode(&encoded).unwrap(), message);
    // A negative number is a 10-byte two's-complement varint.
    let minus_three = ProtoMessage::new(vec![ProtoField::varint(2, -3i64 as u64)]).encode();
    assert_eq!(minus_three, [0x10, 0xFD, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    assert_eq!(ProtoMessage::new(vec![ProtoField::varint(2, 300)]).encode(), [0x10, 0xAC, 0x02]);
}

#[test]
fn packed_varints() {
    let values = [0, 300, 1 << 40, u64::MAX];
    assert_eq!(decode_packed_varints(&encode_packed_varints(&values)).unwrap(), values);
    assert_eq!(decode_packed_varints(&[]).unwrap(), []);
    assert!(decode_packed_varints(&[0x80]).is_err());
}

#[test]
fn all_wire_types_round_trip_in_order() {
    let message = ProtoMessage::new(vec![
        ProtoField::new(5, ProtoValue::Fixed32(0xDEAD_BEEF)),
        ProtoField::bytes(1, vec![1, 2, 3]),
        ProtoField::new(9, ProtoValue::Fixed64(0x0102_0304_0506_0708)),
        ProtoField::varint(1, 7),
        ProtoField::bytes(MAX_FIELD_NUMBER, Vec::new()),
    ]);
    let encoded = message.encode();
    assert_eq!(encoded[..5], [0x2D, 0xEF, 0xBE, 0xAD, 0xDE], "fixed32 is little-endian");
    assert_eq!(ProtoMessage::decode(&encoded).unwrap(), message);
}

#[test]
fn rejects_malformed() {
    let bad: Vec<Vec<u8>> = vec![
        vec![0x0A, 50, 1, 2, 3],          // length runs past the end
        vec![0x08],                       // truncated varint value
        vec![0x80],                       // truncated key
        vec![0x0B],                       // group, wire type 3
        vec![0x0C],                       // wire type 4
        vec![0x07, 0x00],                 // wire type 7
        vec![0x00, 0x01],                 // field number 0
        vec![0x0D, 1, 2, 3],              // truncated fixed32
        vec![0x09, 1, 2, 3],              // truncated fixed64
        vec![0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01], // length overflow
        [vec![0x08], vec![0xFF; 11]].concat(), // varint too long
        vec![0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x02], // varint over 64 bits
        vec![0x80, 0x80, 0x80, 0x80, 0x40, 0x00], // field number above the limit
    ];
    for bytes in bad {
        let result = ProtoMessage::decode(&bytes);
        assert!(matches!(result, Err(SaveKitError::CorruptProtobuf(_))), "{bytes:02X?}: {result:?}");
    }
}

#[test]
fn set_single_semantics() {
    let mut m = ProtoMessage::new(vec![
        ProtoField::varint(1, 1),
        ProtoField::varint(3, 10),
        ProtoField::varint(5, 5),
        ProtoField::varint(3, 11),
    ]);
    m.set_single(3, ProtoValue::Varint(99));
    assert_eq!(numbers(&m), [1, 3, 5]);
    assert_eq!(m.values(3).collect::<Vec<_>>(), [&ProtoValue::Varint(99)]);
    m.set_single(4, ProtoValue::Varint(4));
    assert_eq!(numbers(&m), [1, 3, 4, 5]);
    m.set_single(9, ProtoValue::Varint(9));
    assert_eq!(numbers(&m), [1, 3, 4, 5, 9]);
}

#[test]
fn push_repeated_keeps_entries_together() {
    let mut m = ProtoMessage::new(vec![ProtoField::varint(1, 1), ProtoField::varint(5, 50), ProtoField::varint(9, 9)]);
    assert_eq!(m.push_repeated(5, ProtoValue::Varint(51)), 1);
    assert_eq!(m.push_repeated(3, ProtoValue::Varint(30)), 0);
    assert_eq!(m.push_repeated(12, ProtoValue::Varint(120)), 0);
    assert_eq!(m.push_repeated(5, ProtoValue::Varint(52)), 2);
    assert_eq!(numbers(&m), [1, 3, 5, 5, 5, 9, 12]);
    assert_eq!(m.values(5).filter_map(ProtoValue::as_varint).collect::<Vec<_>>(), [50, 51, 52]);
    assert_eq!(m.count(5), 3);
}

#[test]
fn accessors_read_the_first_entry() {
    let m = ProtoMessage::new(vec![
        ProtoField::bytes(2, "text"),
        ProtoField::varint(2, 8),
        ProtoField::varint(3, 9),
    ]);
    assert_eq!(m.bytes(2), Some(&b"text"[..]));
    assert_eq!(m.varint(2), None, "the first entry of field 2 is not a varint");
    assert_eq!(m.varint(3), Some(9));
    assert_eq!(m.bytes(3), None);
    assert_eq!(m.first(4), None);
}
