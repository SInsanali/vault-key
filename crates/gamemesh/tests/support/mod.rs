//! A tiny writer of UE3 packages (version 832, licensee 46) for tests: just the header fields
//! the reader uses, the name, import and export tables, and export bodies made of tagged
//! properties followed by native data.
#![allow(dead_code)]

use gamedata::package::UPackage;

#[derive(Clone, Default)]
pub struct Pkg {
    names: Vec<String>,
    /// Class names; each becomes an import `Core.Class <name>`.
    classes: Vec<String>,
    /// (class reference, outer reference, name index, body).
    pub exports: Vec<(i32, i32, i32, Vec<u8>)>,
}

impl Pkg {
    pub fn new() -> Self {
        let mut p = Self::default();
        for fixed in ["None", "Core", "Class"] {
            p.name(fixed);
        }
        p
    }

    /// The index of a name, added if new.
    pub fn name(&mut self, s: &str) -> i32 {
        match self.names.iter().position(|n| n == s) {
            Some(i) => i as i32,
            None => {
                self.names.push(s.to_string());
                self.names.len() as i32 - 1
            }
        }
    }

    pub fn fname(&mut self, s: &str) -> Vec<u8> {
        [self.name(s).to_le_bytes(), 0i32.to_le_bytes()].concat()
    }

    fn class(&mut self, class: &str) -> i32 {
        self.name(class);
        let at = self.classes.iter().position(|c| c == class).unwrap_or_else(|| {
            self.classes.push(class.to_string());
            self.classes.len() - 1
        });
        -(at as i32 + 1)
    }

    /// Adds an export and returns its object reference.
    pub fn export(&mut self, class: &str, outer: i32, name: &str, body: Vec<u8>) -> i32 {
        let (class, name) = (self.class(class), self.name(name));
        self.exports.push((class, outer, name, body));
        self.exports.len() as i32
    }

    /// One tagged property: name, type, size, array index, the type's extra bytes, the value.
    pub fn tag(&mut self, name: &str, kind: &str, index: i32, extra: &[u8], value: &[u8]) -> Vec<u8> {
        let size = if kind == "BoolProperty" { 0 } else { value.len() as i32 };
        [&self.fname(name)[..], &self.fname(kind), &size.to_le_bytes(), &index.to_le_bytes(), extra, value].concat()
    }

    pub fn int(&mut self, name: &str, v: i32) -> Vec<u8> {
        self.tag(name, "IntProperty", 0, &[], &v.to_le_bytes())
    }

    pub fn float(&mut self, name: &str, v: f32) -> Vec<u8> {
        self.tag(name, "FloatProperty", 0, &[], &v.to_le_bytes())
    }

    pub fn object(&mut self, name: &str, reference: i32) -> Vec<u8> {
        self.tag(name, "ObjectProperty", 0, &[], &reference.to_le_bytes())
    }

    pub fn name_at(&mut self, name: &str, index: i32, value: &str) -> Vec<u8> {
        let value = self.fname(value);
        self.tag(name, "NameProperty", index, &[], &value)
    }

    pub fn boolean(&mut self, name: &str, v: bool) -> Vec<u8> {
        self.tag(name, "BoolProperty", 0, &[], &[u8::from(v)])
    }

    pub fn enumeration(&mut self, name: &str, enum_type: &str, value: &str) -> Vec<u8> {
        let (enum_type, value) = (self.fname(enum_type), self.fname(value));
        self.tag(name, "ByteProperty", 0, &enum_type, &value)
    }

    /// An array of structs; each element is its tags (the terminator is added here).
    pub fn structs(&mut self, name: &str, elements: &[Vec<u8>]) -> Vec<u8> {
        let mut value = (elements.len() as i32).to_le_bytes().to_vec();
        for element in elements {
            value.extend(element);
            value.extend(self.fname("None"));
        }
        self.tag(name, "ArrayProperty", 0, &[], &value)
    }

    /// A struct serialised as raw bytes (`LinearColor`, …).
    pub fn raw_struct(&mut self, name: &str, struct_name: &str, bytes: &[u8]) -> Vec<u8> {
        let struct_name = self.fname(struct_name);
        self.tag(name, "StructProperty", 0, &struct_name, bytes)
    }

    /// A struct serialised as tagged properties.
    pub fn tagged_struct(&mut self, name: &str, struct_name: &str, tags: &[u8]) -> Vec<u8> {
        let value = [tags, &self.fname("None")].concat();
        self.raw_struct(name, struct_name, &value)
    }

    /// An export body: net index, tags, terminator, native data.
    pub fn body(&mut self, tags: &[Vec<u8>], native: &[u8]) -> Vec<u8> {
        [&0i32.to_le_bytes()[..], &tags.concat(), &self.fname("None"), native].concat()
    }

    pub fn build(&self) -> Vec<u8> {
        let i32s = |values: &[i32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
        let mut names = Vec::new();
        for name in &self.names {
            names.extend((name.len() as i32 + 1).to_le_bytes());
            names.extend(name.as_bytes());
            names.extend([0u8; 9]); // the terminator, then 8 bytes of flags
        }
        let mut imports = Vec::new();
        for class in &self.classes {
            let name = self.names.iter().position(|n| n == class).unwrap_or(0) as i32;
            imports.extend(i32s(&[1, 0, 2, 0, 0, name, 0])); // Core, Class, no outer, the class name
        }
        const HEADER: usize = 4 + 4 + 4 + 9 + 4 + 24;
        let name_offset = HEADER;
        let import_offset = name_offset + names.len();
        let export_offset = import_offset + imports.len();
        let mut body_offset = export_offset + self.exports.len() * 68;
        let (mut exports, mut bodies) = (Vec::new(), Vec::new());
        for (class, outer, name, body) in &self.exports {
            exports.extend(i32s(&[*class, 0, *outer, *name, 0, 0, 0, 0, body.len() as i32, body_offset as i32, 0, 0]));
            exports.extend([0u8; 20]); // GUID, package flags
            body_offset += body.len();
            bodies.extend(body);
        }
        let mut out = vec![0xC1, 0x83, 0x2A, 0x9E, 0x40, 0x03, 0x2E, 0x00];
        out.extend(i32s(&[0, 5]));
        out.extend(b"None\0");
        out.extend(i32s(&[0, self.names.len() as i32, name_offset as i32, self.exports.len() as i32, export_offset as i32]));
        out.extend(i32s(&[self.classes.len() as i32, import_offset as i32]));
        assert_eq!(out.len(), HEADER);
        [out, names, imports, exports, bodies].concat()
    }

    pub fn parse(&self) -> Result<UPackage, gamedata::GameDataError> {
        UPackage::parse("test", self.build())
    }
}

/// The native data of a small skeletal mesh: `bones` as (name index, parent), one section and
/// one chunk, 16-bit indices, one half-precision texture-coordinate set.
pub fn mesh_native(positions: &[[f32; 3]], indices: &[u16], bones: &[(i32, i32)]) -> Vec<u8> {
    let mut b = Vec::new();
    let f = |b: &mut Vec<u8>, v: f32| b.extend(v.to_le_bytes());
    let i = |b: &mut Vec<u8>, v: i32| b.extend(v.to_le_bytes());
    for _ in 0..7 {
        f(&mut b, 1.0); // bounds
    }
    i(&mut b, 1);
    i(&mut b, 0); // one null material
    b.extend([0u8; 24]);
    i(&mut b, bones.len() as i32);
    for &(name, parent) in bones {
        i(&mut b, name);
        i(&mut b, 0);
        i(&mut b, 0);
        for q in [0.0, 0.0, 0.0, 1.0] {
            f(&mut b, q);
        }
        b.extend([0u8; 12]);
        i(&mut b, 0);
        i(&mut b, parent);
        i(&mut b, -1);
    }
    i(&mut b, 1); // depth
    i(&mut b, 1); // LODs
    i(&mut b, 1); // sections
    b.extend([0, 0, 0, 0]);
    i(&mut b, 0);
    i(&mut b, (indices.len() / 3) as i32);
    b.push(0);
    i(&mut b, 1);
    b.push(2);
    i(&mut b, 2);
    i(&mut b, indices.len() as i32);
    for x in indices {
        b.extend(x.to_le_bytes());
    }
    i(&mut b, 0); // active bones
    i(&mut b, 1); // chunks
    i(&mut b, 0);
    i(&mut b, 0);
    i(&mut b, 0);
    i(&mut b, bones.len() as i32);
    for n in 0..bones.len() as u16 {
        b.extend(n.to_le_bytes());
    }
    i(&mut b, positions.len() as i32);
    i(&mut b, 0);
    i(&mut b, 4);
    i(&mut b, 0); // size
    i(&mut b, positions.len() as i32);
    i(&mut b, 0); // required bones
    b.extend([0u8; 16]); // empty bulk data
    i(&mut b, 1);
    i(&mut b, 1);
    i(&mut b, 0);
    i(&mut b, 0);
    b.extend([0u8; 24]);
    i(&mut b, 32);
    i(&mut b, positions.len() as i32);
    for (n, p) in positions.iter().enumerate() {
        b.extend([0, 0, 0, 0]);
        b.extend([0, 127, 127, 0]); // normal −x, towards the default camera
        b.extend([(n % bones.len().max(1)) as u8, 0, 0, 0]);
        b.extend([255, 0, 0, 0]);
        for v in p {
            f(&mut b, *v);
        }
        b.extend(0x3C00u16.to_le_bytes()); // u = 1
        b.extend(0x3800u16.to_le_bytes()); // v = 0.5
    }
    b.extend([7u8; 20]); // whatever follows the vertex buffer is not read
    b
}

/// A small deterministic random number generator (xorshift).
pub struct Rng(pub u32);

impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n.max(1)
    }
}
