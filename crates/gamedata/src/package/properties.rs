//! The tagged-property reader: an object's body is an i32 net index, then tagged properties
//! until the name `None`.

use super::reader::ByteReader;
use super::upackage::UPackage;
use crate::error::{GameDataError, Result};

/// Structs the engine serialises as raw bytes instead of tagged properties.
const NATIVE_STRUCTS: &[&str] = &[
    "Color",
    "LinearColor",
    "Vector",
    "Vector2D",
    "Rotator",
    "Guid",
    "Quat",
    "Box",
    "Plane",
    "Matrix",
    "IntPoint",
];
const MAX_STRUCT_DEPTH: usize = 16;
const MAX_PROPERTIES: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Int(i32),
    Float(f32),
    Bool(bool),
    Byte(u8),
    /// A `ByteProperty` holding an enum value by name.
    EnumName(String),
    Name(String),
    Str(String),
    /// An object reference (see [`UPackage`]).
    Object(i32),
    /// A struct serialised as tagged properties.
    Struct(PropertyList),
    /// A native struct, or one whose bytes do not parse as tagged properties.
    StructRaw {
        name: String,
        bytes: Vec<u8>,
    },
    /// An array: the element count and the undecoded element bytes. The element type is not
    /// stored in the file, so use [`UPackage::object_array`], [`UPackage::string_array`] or
    /// [`UPackage::struct_array`] for the shape the property is known to have.
    Array {
        count: usize,
        bytes: Vec<u8>,
    },
    Unknown {
        type_name: String,
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    /// The element index for static arrays such as `PathComponentNames[6]`; otherwise 0.
    pub array_index: usize,
    pub value: PropertyValue,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PropertyList {
    pub items: Vec<Property>,
}

impl PropertyList {
    /// The value of property `name` (static-array element 0).
    pub fn get(&self, name: &str) -> Option<&PropertyValue> {
        self.get_at(name, 0)
    }

    /// The value of element `index` of static-array property `name`.
    pub fn get_at(&self, name: &str, index: usize) -> Option<&PropertyValue> {
        self.items
            .iter()
            .find(|p| p.name == name && p.array_index == index)
            .map(|p| &p.value)
    }

    pub fn int(&self, name: &str) -> Option<i32> {
        match self.get(name)? {
            PropertyValue::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        match self.get(name)? {
            PropertyValue::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            PropertyValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn string(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            PropertyValue::Str(v) => Some(v),
            _ => None,
        }
    }

    pub fn object(&self, name: &str) -> Option<i32> {
        self.object_at(name, 0)
    }

    pub fn object_at(&self, name: &str, index: usize) -> Option<i32> {
        match self.get_at(name, index)? {
            PropertyValue::Object(v) => Some(*v),
            _ => None,
        }
    }

    pub fn name(&self, name: &str) -> Option<&str> {
        self.name_at(name, 0)
    }

    pub fn name_at(&self, name: &str, index: usize) -> Option<&str> {
        match self.get_at(name, index)? {
            PropertyValue::Name(v) => Some(v),
            _ => None,
        }
    }

    pub fn enum_name(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            PropertyValue::EnumName(v) => Some(v),
            _ => None,
        }
    }

    pub fn struct_fields(&self, name: &str) -> Option<&PropertyList> {
        match self.get(name)? {
            PropertyValue::Struct(v) => Some(v),
            _ => None,
        }
    }
}

fn array_parts(value: &PropertyValue) -> Result<(usize, &[u8])> {
    match value {
        PropertyValue::Array { count, bytes } => Ok((*count, bytes)),
        _ => Err(GameDataError::UnexpectedData("not an array".into())),
    }
}

impl UPackage {
    /// The tagged properties of export `index`.
    pub fn properties_of_export(&self, index: usize) -> Result<PropertyList> {
        let e = self
            .exports
            .get(index)
            .ok_or_else(|| GameDataError::BadReference(format!("export index {index}")))?;
        let end = e
            .serial_offset
            .checked_add(e.serial_size)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| {
                GameDataError::Truncated(format!("{}: export {} data", self.name, e.object_name))
            })?;
        let mut r = ByteReader::with_range(&self.bytes, e.serial_offset, end);
        r.skip(4)?; // netIndex
        self.read_tags(&mut r, 0)
    }

    /// The tagged properties of export `index`, and the bytes that follow them in the export's
    /// data: the class's own binary serialisation (mesh buffers, texture mips). Empty for the
    /// many classes that store nothing but properties.
    pub fn properties_and_native_data(&self, index: usize) -> Result<(PropertyList, &[u8])> {
        let e = self
            .exports
            .get(index)
            .ok_or_else(|| GameDataError::BadReference(format!("export index {index}")))?;
        let end = e
            .serial_offset
            .checked_add(e.serial_size)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| {
                GameDataError::Truncated(format!("{}: export {} data", self.name, e.object_name))
            })?;
        let mut r = ByteReader::with_range(&self.bytes, e.serial_offset, end);
        r.skip(4)?; // netIndex
        let list = self.read_tags(&mut r, 0)?;
        Ok((list, &self.bytes[r.position()..end]))
    }

    /// Decodes an array of object references (4 bytes each).
    pub fn object_array(&self, value: &PropertyValue) -> Result<Vec<i32>> {
        let (count, bytes) = array_parts(value)?;
        if count.checked_mul(4) != Some(bytes.len()) {
            return Err(GameDataError::UnexpectedData(format!(
                "object array size {} for {count} elements",
                bytes.len()
            )));
        }
        let mut r = ByteReader::new(bytes);
        (0..count).map(|_| r.i32()).collect()
    }

    /// Decodes an array of FStrings.
    pub fn string_array(&self, value: &PropertyValue) -> Result<Vec<String>> {
        let (count, bytes) = array_parts(value)?;
        // Each element needs at least its 4-byte length.
        if count > bytes.len() / 4 {
            return Err(GameDataError::UnexpectedData(format!(
                "string array count {count}"
            )));
        }
        let mut r = ByteReader::new(bytes);
        let out = (0..count)
            .map(|_| r.fstring())
            .collect::<Result<Vec<_>>>()?;
        if !r.at_end() {
            return Err(GameDataError::UnexpectedData(
                "string array trailing bytes".into(),
            ));
        }
        Ok(out)
    }

    /// Decodes an array of structs, each serialised as tagged properties.
    pub fn struct_array(&self, value: &PropertyValue) -> Result<Vec<PropertyList>> {
        let (count, bytes) = array_parts(value)?;
        // Each element needs at least the 8-byte `None` name.
        if count > bytes.len() / 8 {
            return Err(GameDataError::UnexpectedData(format!(
                "struct array count {count}"
            )));
        }
        let mut r = ByteReader::new(bytes);
        let out = (0..count)
            .map(|_| self.read_tags(&mut r, 0))
            .collect::<Result<Vec<_>>>()?;
        if !r.at_end() {
            return Err(GameDataError::UnexpectedData(
                "struct array trailing bytes".into(),
            ));
        }
        Ok(out)
    }

    /// Reads tags until `None`, bounded by the reader's end.
    fn read_tags(&self, r: &mut ByteReader<'_>, depth: usize) -> Result<PropertyList> {
        if depth > MAX_STRUCT_DEPTH {
            return Err(GameDataError::UnexpectedData(
                "property nesting too deep".into(),
            ));
        }
        let mut items = Vec::new();
        loop {
            let name = self.read_name(r)?;
            if name == "None" {
                break;
            }
            if items.len() >= MAX_PROPERTIES {
                return Err(GameDataError::UnexpectedData("too many properties".into()));
            }
            let type_name = self.read_name(r)?;
            let what = format!("tag {name}");
            let size = r.i32_size(&what)?;
            let array_index = r.i32_size(&what)?;
            let mut struct_name = String::new();
            let mut bool_value = false;
            match type_name.as_str() {
                "StructProperty" => struct_name = self.read_name(r)?,
                // The enum's type name; the value follows.
                "ByteProperty" => {
                    self.read_name(r)?;
                }
                // The value byte is stored in the tag and is not counted in `size`.
                "BoolProperty" => bool_value = r.u8()? != 0,
                _ => {}
            }
            // The value is decoded from its own slice, so a short or odd value can never
            // disturb the position of the next tag.
            let bytes = r
                .take(size)
                .map_err(|_| GameDataError::Truncated(format!("tag {name}: size {size}")))?;
            let value =
                self.read_value(bytes, &type_name, struct_name, bool_value, &name, depth)?;
            items.push(Property {
                name,
                type_name,
                array_index,
                value,
            });
        }
        Ok(PropertyList { items })
    }

    /// Decodes one value from `bytes`, which is exactly the value (the tag's `size` bytes).
    fn read_value(
        &self,
        bytes: &[u8],
        type_name: &str,
        struct_name: String,
        bool_value: bool,
        tag: &str,
        depth: usize,
    ) -> Result<PropertyValue> {
        let size = bytes.len();
        let sized = |expected: usize| -> Result<()> {
            if size == expected {
                Ok(())
            } else {
                Err(GameDataError::UnexpectedData(format!(
                    "tag {tag}: {type_name} size {size}"
                )))
            }
        };
        let mut value = ByteReader::new(bytes);
        Ok(match type_name {
            "IntProperty" => {
                sized(4)?;
                PropertyValue::Int(value.i32()?)
            }
            "FloatProperty" => {
                sized(4)?;
                PropertyValue::Float(value.f32()?)
            }
            "ObjectProperty" => {
                sized(4)?;
                PropertyValue::Object(value.i32()?)
            }
            "NameProperty" => {
                sized(8)?;
                PropertyValue::Name(self.read_name(&mut value)?)
            }
            "BoolProperty" => PropertyValue::Bool(bool_value),
            "ByteProperty" => {
                if size == 8 {
                    PropertyValue::EnumName(self.read_name(&mut value)?)
                } else {
                    sized(1)?;
                    PropertyValue::Byte(value.u8()?)
                }
            }
            "StrProperty" => {
                let s = value.fstring()?;
                if !value.at_end() {
                    return Err(GameDataError::UnexpectedData(format!(
                        "tag {tag}: string does not fill size"
                    )));
                }
                PropertyValue::Str(s)
            }
            "ArrayProperty" => {
                if size < 4 {
                    return Err(GameDataError::UnexpectedData(format!(
                        "tag {tag}: array size {size}"
                    )));
                }
                let count = value.i32_size(&format!("tag {tag}: array count"))?;
                PropertyValue::Array {
                    count,
                    bytes: bytes[4..].to_vec(),
                }
            }
            "StructProperty" => {
                if !NATIVE_STRUCTS.contains(&struct_name.as_str()) {
                    let mut sub = ByteReader::new(bytes);
                    if let Ok(list) = self.read_tags(&mut sub, depth + 1) {
                        if sub.at_end() {
                            return Ok(PropertyValue::Struct(list));
                        }
                    }
                }
                PropertyValue::StructRaw {
                    name: struct_name,
                    bytes: bytes.to_vec(),
                }
            }
            _ => PropertyValue::Unknown {
                type_name: type_name.to_string(),
                bytes: bytes.to_vec(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fname, fstring, le32, p, property_body, raw_tag, Pkg, P, V};

    /// The name table is `None` followed by these, so index = position + 1.
    const NAMES: &[&str] = &[
        "Int",
        "IntProperty",
        "Flt",
        "FloatProperty",
        "Str",
        "StrProperty",
        "Nm",
        "NameProperty",
        "Obj",
        "ObjectProperty",
        "En",
        "ByteProperty",
        "Byt",
        "Bl",
        "BoolProperty",
        "RARITY_Rare",
        "WT_Pistol",
        "Col",
        "StructProperty",
        "Color",
        "Arr",
        "ArrayProperty",
        "MinLevel",
        "Inner",
        "EnumX",
        "Odd",
        "QWordProperty",
    ];

    fn ix(name: &str) -> i32 {
        if name == "None" {
            0
        } else {
            NAMES.iter().position(|n| *n == name).unwrap() as i32 + 1
        }
    }

    fn tag(name: &str, type_name: &str, extra: &[u8], value: &[u8]) -> Vec<u8> {
        raw_tag(ix(name), ix(type_name), None, 0, extra, value)
    }

    /// A package with one export whose body is `body`.
    fn package_with_body(body: Vec<u8>) -> UPackage {
        let mut b = Pkg::with_names(NAMES);
        b.raw_export(0, 0, "Obj", body);
        UPackage::parse("t", b.build()).unwrap()
    }

    fn props(tags: &[Vec<u8>]) -> PropertyList {
        package_with_body(property_body(tags))
            .properties_of_export(0)
            .unwrap()
    }

    #[test]
    fn parses_every_tag_type() {
        let list = props(&[
            tag("Int", "IntProperty", &[], &le32(7)),
            tag("Flt", "FloatProperty", &[], &10f32.to_le_bytes()),
            tag("Str", "StrProperty", &[], &fstring("Infinity")),
            tag("Nm", "NameProperty", &[], &fname(ix("WT_Pistol"), 0)),
            tag("Obj", "ObjectProperty", &[], &le32(-1)),
            tag(
                "En",
                "ByteProperty",
                &fname(ix("EnumX"), 0),
                &fname(ix("RARITY_Rare"), 0),
            ),
            tag("Byt", "ByteProperty", &fname(0, 0), &[3]),
            raw_tag(ix("Bl"), ix("BoolProperty"), Some(0), 0, &[], &[1]),
            tag("Odd", "QWordProperty", &[], &[1, 2, 3, 4, 5, 6, 7, 8]),
        ]);
        assert_eq!(list.int("Int"), Some(7));
        assert_eq!(list.float("Flt"), Some(10.0));
        assert_eq!(list.string("Str"), Some("Infinity"));
        assert_eq!(list.name("Nm"), Some("WT_Pistol"));
        assert_eq!(list.object("Obj"), Some(-1));
        assert_eq!(list.enum_name("En"), Some("RARITY_Rare"));
        assert_eq!(list.get("Byt"), Some(&PropertyValue::Byte(3)));
        assert_eq!(list.bool("Bl"), Some(true));
        assert_eq!(
            list.get("Odd"),
            Some(&PropertyValue::Unknown {
                type_name: "QWordProperty".into(),
                bytes: vec![1, 2, 3, 4, 5, 6, 7, 8]
            })
        );
        // Accessors are typed: asking for the wrong type, or a missing name, gives nothing.
        assert_eq!(list.int("Flt"), None);
        assert_eq!(list.string("Missing"), None);
        assert_eq!(list.items.len(), 9);
        assert_eq!(list.items[0].type_name, "IntProperty");
    }

    #[test]
    fn native_data_is_what_follows_the_properties() {
        let mut body = property_body(&[tag("Int", "IntProperty", &[], &le32(7))]);
        body.extend([9, 8, 7]);
        let pkg = package_with_body(body);
        let (list, tail) = pkg.properties_and_native_data(0).unwrap();
        assert_eq!(list.int("Int"), Some(7));
        assert_eq!(tail, [9, 8, 7]);
        // Nothing after the terminator gives an empty tail; a bad index is an error.
        let pkg = package_with_body(property_body(&[]));
        assert!(pkg.properties_and_native_data(0).unwrap().1.is_empty());
        assert!(pkg.properties_and_native_data(5).is_err());
    }

    #[test]
    fn numbered_names_get_a_suffix() {
        let list = props(&[tag("Nm", "NameProperty", &[], &fname(ix("Inner"), 3))]);
        assert_eq!(list.name("Nm"), Some("Inner_2"));
    }

    #[test]
    fn bool_value_byte_is_not_counted_in_size() {
        let list = props(&[
            raw_tag(ix("Bl"), ix("BoolProperty"), Some(0), 0, &[], &[0]),
            tag("Int", "IntProperty", &[], &le32(5)),
        ]);
        assert_eq!((list.bool("Bl"), list.int("Int")), (Some(false), Some(5)));
    }

    #[test]
    fn static_array_indices_are_kept() {
        let tags: Vec<Vec<u8>> = (0..6)
            .map(|i| {
                let value = fname(ix(if i == 5 { "Obj" } else { "Nm" }), 0);
                raw_tag(ix("Inner"), ix("NameProperty"), None, i, &[], &value)
            })
            .collect();
        let list = props(&tags);
        assert_eq!(list.name_at("Inner", 5), Some("Obj"));
        assert_eq!(list.name_at("Inner", 0), Some("Nm"));
        assert_eq!(list.name("Inner"), Some("Nm"));
        assert_eq!(list.name_at("Inner", 6), None);
    }

    #[test]
    fn native_structs_stay_raw() {
        let raw = [0x0B, 0xD2, 0x3D, 0xFF];
        let list = props(&[tag("Col", "StructProperty", &fname(ix("Color"), 0), &raw)]);
        assert_eq!(
            list.get("Col"),
            Some(&PropertyValue::StructRaw {
                name: "Color".into(),
                bytes: raw.to_vec()
            })
        );
    }

    #[test]
    fn tagged_struct_parses_and_falls_back_to_raw() {
        let mut inner = tag("MinLevel", "IntProperty", &[], &le32(9));
        inner.extend(fname(0, 0));
        let mut trailing = inner.clone();
        trailing.push(0);
        let list = props(&[
            tag("Col", "StructProperty", &fname(ix("Inner"), 0), &inner),
            tag(
                "Arr",
                "StructProperty",
                &fname(ix("Inner"), 0),
                &[1, 2, 3, 4, 5],
            ),
            // Tags that parse but do not fill the value are not a tagged struct either.
            tag("Odd", "StructProperty", &fname(ix("Inner"), 0), &trailing),
        ]);
        assert_eq!(
            list.struct_fields("Col").and_then(|s| s.int("MinLevel")),
            Some(9)
        );
        assert_eq!(
            list.get("Arr"),
            Some(&PropertyValue::StructRaw {
                name: "Inner".into(),
                bytes: vec![1, 2, 3, 4, 5]
            })
        );
        assert!(matches!(
            list.get("Odd"),
            Some(PropertyValue::StructRaw { .. })
        ));
        assert_eq!(list.struct_fields("Arr"), None);
    }

    #[test]
    fn struct_nesting_is_limited() {
        // Each level wraps the previous one in a struct tag.
        let mut value = fname(0, 0);
        for _ in 0..40 {
            let mut outer = tag("Col", "StructProperty", &fname(ix("Inner"), 0), &value);
            outer.extend(fname(0, 0));
            value = outer;
        }
        let list = props(&[tag("Col", "StructProperty", &fname(ix("Inner"), 0), &value)]);
        // Parsing stops descending at the limit and keeps the rest as raw bytes.
        let mut depth = 0;
        let mut current = list.get("Col");
        while let Some(PropertyValue::Struct(fields)) = current {
            depth += 1;
            current = fields.get("Col");
        }
        assert!(matches!(current, Some(PropertyValue::StructRaw { .. })));
        assert_eq!(depth, MAX_STRUCT_DEPTH);
    }

    #[test]
    fn array_accessors() {
        let element = |n: i32| {
            let mut e = tag("MinLevel", "IntProperty", &[], &le32(n));
            e.extend(fname(0, 0));
            e
        };
        let structs = [le32(2), element(1), element(2)].concat();
        let objects = [le32(2), le32(1), le32(-3)].concat();
        let strings = [le32(2), fstring("a"), fstring("bc")].concat();
        let short = [le32(3), le32(1)].concat();
        let pkg = package_with_body(property_body(&[
            tag("Arr", "ArrayProperty", &[], &structs),
            tag("Obj", "ArrayProperty", &[], &objects),
            tag("Str", "ArrayProperty", &[], &strings),
            tag("Nm", "ArrayProperty", &[], &short),
            tag("Int", "IntProperty", &[], &le32(1)),
        ]));
        let list = pkg.properties_of_export(0).unwrap();
        let get = |name: &str| list.get(name).unwrap();

        let decoded = pkg.struct_array(get("Arr")).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[1].int("MinLevel"), Some(2));
        assert_eq!(pkg.object_array(get("Obj")).unwrap(), [1, -3]);
        assert_eq!(pkg.string_array(get("Str")).unwrap(), ["a", "bc"]);

        // A count that the bytes cannot hold, the wrong shape, and a non-array.
        assert!(pkg.object_array(get("Nm")).is_err());
        assert!(pkg.string_array(get("Nm")).is_err());
        assert!(pkg.struct_array(get("Nm")).is_err());
        assert!(pkg.object_array(get("Str")).is_err());
        assert!(pkg.struct_array(get("Obj")).is_err());
        assert!(pkg.string_array(get("Arr")).is_err()); // trailing bytes
        assert!(pkg.object_array(get("Int")).is_err());
    }

    #[test]
    fn bad_tags_are_errors() {
        let fails = |body: Vec<u8>| package_with_body(body).properties_of_export(0).is_err();
        // A size larger than the data.
        let oversized = raw_tag(ix("Int"), ix("IntProperty"), Some(4096), 0, &[], &le32(1));
        assert!(fails(property_body(&[oversized])));
        // Negative size and array index.
        assert!(fails(property_body(&[raw_tag(
            ix("Int"),
            ix("IntProperty"),
            Some(-4),
            0,
            &[],
            &le32(1)
        )])));
        assert!(fails(property_body(&[raw_tag(
            ix("Int"),
            ix("IntProperty"),
            None,
            -1,
            &[],
            &le32(1)
        )])));
        // A fixed-size type with the wrong size.
        assert!(fails(property_body(&[tag(
            "Int",
            "IntProperty",
            &[],
            &[1, 2]
        )])));
        assert!(fails(property_body(&[tag(
            "Byt",
            "ByteProperty",
            &fname(0, 0),
            &[1, 2]
        )])));
        // A string that does not fill its value, an array without a count, a negative count.
        let mut long = fstring("a");
        long.push(7);
        assert!(fails(property_body(&[tag(
            "Str",
            "StrProperty",
            &[],
            &long
        )])));
        assert!(fails(property_body(&[tag(
            "Arr",
            "ArrayProperty",
            &[],
            &[1, 2]
        )])));
        assert!(fails(property_body(&[tag(
            "Arr",
            "ArrayProperty",
            &[],
            &le32(-1)
        )])));
        // A name index outside the table.
        assert!(fails([le32(0), fname(500, 0)].concat()));
        // No `None` terminator, and a body too short for its net index.
        assert!(fails([le32(0), fname(ix("Int"), 0)].concat()));
        assert!(fails(vec![0, 0]));
        // An export index outside the table.
        let pkg = package_with_body(property_body(&[]));
        assert!(matches!(
            pkg.properties_of_export(5),
            Err(GameDataError::BadReference(_))
        ));
    }

    #[test]
    fn builder_properties_read_back() {
        let mut b = Pkg::new();
        b.export(
            0,
            0,
            "Obj",
            &[
                p("A", V::Int(3)),
                p(
                    "B",
                    V::Struct(vec![p("C", V::Float(1.5)), p("D", V::Obj(-2))]),
                ),
                p("E", V::Structs(vec![vec![P("F", 2, V::Name("G"))], vec![]])),
                p("H", V::Enum("WT_SMG")),
                p("I", V::Color([1, 2, 3, 4])),
                p("J", V::Strs(vec!["x"])),
                p("K", V::Objs(vec![1])),
                p("L", V::Bool(true)),
                p("M", V::Str("s")),
                p("N", V::Byte(9)),
            ],
        );
        let pkg = UPackage::parse("t", b.build()).unwrap();
        let list = pkg.properties_of_export(0).unwrap();
        assert_eq!(list.int("A"), Some(3));
        let inner = list.struct_fields("B").unwrap();
        assert_eq!((inner.float("C"), inner.object("D")), (Some(1.5), Some(-2)));
        let elements = pkg.struct_array(list.get("E").unwrap()).unwrap();
        assert_eq!(elements[0].name_at("F", 2), Some("G"));
        assert!(elements[1].items.is_empty());
        assert_eq!(list.enum_name("H"), Some("WT_SMG"));
        assert!(matches!(
            list.get("I"),
            Some(PropertyValue::StructRaw { .. })
        ));
        assert_eq!(pkg.string_array(list.get("J").unwrap()).unwrap(), ["x"]);
        assert_eq!(pkg.object_array(list.get("K").unwrap()).unwrap(), [1]);
        assert_eq!(list.bool("L"), Some(true));
        assert_eq!(list.string("M"), Some("s"));
        assert_eq!(list.get("N"), Some(&PropertyValue::Byte(9)));
    }
}
