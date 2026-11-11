//! The binary serialisation of a `SkeletalMesh` in this game's packages (version 832,
//! licensee 46), as far as LOD 0's geometry. The layout was worked out from the game's own
//! files; see `docs/research/2026-11-09-weapon-pictures-spike.md` §3.

use gamedata::package::UPackage;

use crate::error::{MeshError, Result};
use crate::reader::{half_to_f32, Cursor};

/// One bone of the reference skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone; the root names itself.
    pub parent: usize,
    /// Rotation relative to the parent, as a quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Position relative to the parent.
    pub position: [f32; 3],
}

/// A run of triangles drawn with one material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub material_index: u16,
    pub chunk_index: u16,
    /// Offset of the first index in [`SkeletalMesh::indices`].
    pub base_index: u32,
    pub triangle_count: u32,
}

/// A run of vertices that share a bone map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub base_vertex: u32,
    /// Maps a vertex's bone number to an index into [`SkeletalMesh::bones`].
    pub bone_map: Vec<u16>,
    pub rigid_vertex_count: u32,
    pub soft_vertex_count: u32,
}

/// One vertex in the reference pose, in mesh space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// The first texture-coordinate set.
    pub uv: [f32; 2],
    /// The bone with the largest weight, as an index into [`SkeletalMesh::bones`].
    pub bone: u16,
}

/// LOD 0 of a skeletal mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletalMesh {
    /// Bounding box origin and half-extent, and bounding sphere radius, as stored.
    pub bounds_origin: [f32; 3],
    pub bounds_extent: [f32; 3],
    pub bounds_radius: f32,
    /// Material object references (see [`UPackage`]); 0 means "set at run time".
    pub materials: Vec<i32>,
    pub bones: Vec<Bone>,
    pub lod_count: usize,
    pub sections: Vec<Section>,
    pub chunks: Vec<Chunk>,
    /// Triangle list: three vertex indices per triangle.
    pub indices: Vec<u32>,
    pub vertices: Vec<Vertex>,
    /// How many texture-coordinate sets each vertex has in the file.
    pub uv_set_count: usize,
    /// Every texture-coordinate set after the first, `uv_set_count - 1` pairs per vertex.
    pub extra_uvs: Vec<[f32; 2]>,
}

/// Bones, sections, chunks and vertices above these counts are treated as corrupt.
const MAX_BONES: usize = 1024;

impl SkeletalMesh {
    /// Reads the mesh stored in export `index` of `pkg`.
    pub fn from_export(pkg: &UPackage, index: usize) -> Result<Self> {
        let class = pkg.class_name_of_export(index);
        if class != "SkeletalMesh" {
            return Err(MeshError::Unsupported(format!("export {index} is a {class}, not a SkeletalMesh")));
        }
        let (_, native) = pkg.properties_and_native_data(index)?;
        Self::parse(native, &pkg.names)
    }

    /// Parses the bytes that follow a SkeletalMesh's tagged properties. `names` is the
    /// package's name table (bone names are stored as indices into it).
    pub fn parse(native: &[u8], names: &[String]) -> Result<Self> {
        let mut c = Cursor::new(native);
        let bounds_origin = c.vec3()?;
        let bounds_extent = c.vec3()?;
        let bounds_radius = c.f32()?;

        let material_count = c.count(4, "materials")?;
        let materials = (0..material_count).map(|_| c.i32()).collect::<Result<Vec<_>>>()?;
        c.skip(12 + 12)?; // origin (vector), rotation origin (rotator)

        let bone_count = c.count(52, "bones")?;
        if bone_count > MAX_BONES {
            return Err(MeshError::Unsupported(format!("{bone_count} bones")));
        }
        let mut bones = Vec::with_capacity(bone_count);
        for _ in 0..bone_count {
            let name = read_name(&mut c, names)?;
            c.skip(4)?; // flags
            let rotation = [c.f32()?, c.f32()?, c.f32()?, c.f32()?];
            let position = c.vec3()?;
            c.skip(4)?; // child count
            let parent = c.i32()?;
            c.skip(4)?; // colour
            let parent = usize::try_from(parent)
                .ok()
                .filter(|&p| p < bone_count)
                .ok_or_else(|| MeshError::Unsupported(format!("bone {name}: parent {parent}")))?;
            bones.push(Bone { name, parent, rotation, position });
        }
        c.skip(4)?; // skeletal depth

        let lod_count = c.count(1, "LOD models")?;
        if lod_count == 0 {
            return Err(MeshError::Unsupported("no LOD models".into()));
        }

        // --- LOD 0 ---
        let section_count = c.count(13, "sections")?;
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let material_index = c.u16()?;
            let chunk_index = c.u16()?;
            let base_index = c.u32()?;
            let triangle_count = c.u32()?;
            c.skip(1)?; // triangle sorting
            sections.push(Section { material_index, chunk_index, base_index, triangle_count });
        }

        // Index buffer: "needs CPU access" flag, index size, then a bulk array (element
        // size, count, elements).
        c.skip(4)?;
        let index_size = usize::from(c.u8()?);
        let element_size = c.i32()?;
        if !(index_size == 2 || index_size == 4) || element_size != index_size as i32 {
            return Err(MeshError::Unsupported(format!("index size {index_size} / element size {element_size}")));
        }
        let index_count = c.count(index_size, "indices")?;
        let raw = c.take(index_count * index_size)?;
        let indices: Vec<u32> = if index_size == 2 {
            raw.as_chunks::<2>().0.iter().map(|b| u32::from(u16::from_le_bytes(*b))).collect()
        } else {
            raw.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect()
        };

        let active_bones = c.count(2, "active bones")?;
        c.skip(active_bones * 2)?;

        let chunk_count = c.count(28, "chunks")?;
        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let base_vertex = c.u32()?;
            // The CPU-side rigid and soft vertex arrays. Cooked packages leave them empty;
            // their element layout is not known here, so anything else is refused.
            for what in ["rigid", "soft"] {
                let n = c.i32()?;
                if n != 0 {
                    return Err(MeshError::Unsupported(format!("{n} CPU-side {what} vertices")));
                }
            }
            let map_count = c.count(2, "bone map")?;
            let bone_map = (0..map_count).map(|_| c.u16()).collect::<Result<Vec<_>>>()?;
            if let Some(bad) = bone_map.iter().find(|&&b| usize::from(b) >= bone_count) {
                return Err(MeshError::Unsupported(format!("bone map entry {bad}")));
            }
            let rigid_vertex_count = c.u32()?;
            let soft_vertex_count = c.u32()?;
            c.skip(4)?; // max bone influences
            chunks.push(Chunk { base_vertex, bone_map, rigid_vertex_count, soft_vertex_count });
        }

        c.skip(4)?; // size
        let vertex_total = c.u32()?;
        let required_bones = c.count(1, "required bones")?;
        c.skip(required_bones)?;

        // Raw point indices: a bulk-data header, with the payload inline unless it is stored
        // elsewhere (flag 1).
        let bulk_flags = c.u32()?;
        c.skip(4)?; // element count
        let bulk_size = c.count(1, "raw point indices")?;
        c.skip(4)?; // offset in file
        if bulk_flags & 1 == 0 {
            c.skip(bulk_size)?;
        }

        // The GPU vertex buffer.
        let uv_set_count = c.count(0, "texture coordinate sets")?;
        let buffer_uv_sets = c.u32()? as usize;
        let full_precision_uvs = c.u32()? != 0;
        c.skip(4)?; // "packed position": set on some meshes, positions are 12 bytes regardless
        c.skip(12 + 12)?; // mesh extension and origin (used only by packed positions)
        if buffer_uv_sets != uv_set_count || !(1..=4).contains(&uv_set_count) {
            return Err(MeshError::Unsupported(format!("{uv_set_count} / {buffer_uv_sets} texture coordinate sets")));
        }
        let uv_size = if full_precision_uvs { 8 } else { 4 };
        let expected = 8 + 4 + 4 + 12 + uv_set_count * uv_size;
        let element_size = c.i32()?;
        if element_size != expected as i32 {
            return Err(MeshError::Unsupported(format!("vertex size {element_size}, expected {expected}")));
        }
        let vertex_count = c.count(expected, "vertices")?;
        if vertex_count != vertex_total as usize {
            return Err(MeshError::Unsupported(format!("{vertex_count} vertices in the buffer, {vertex_total} declared")));
        }

        // Which chunk each vertex belongs to, for its bone map.
        let mut vertices = Vec::with_capacity(vertex_count);
        let mut extra_uvs = Vec::with_capacity(vertex_count * (uv_set_count - 1));
        for v in 0..vertex_count {
            c.skip(4)?; // tangent
            let n = c.take(4)?;
            let normal = [unpack(n[0]), unpack(n[1]), unpack(n[2])];
            let bone_numbers = c.take(4)?;
            let weights = c.take(4)?;
            let position = c.vec3()?;
            let mut uvs = [[0f32; 2]; 4];
            for uv in uvs.iter_mut().take(uv_set_count) {
                *uv = if full_precision_uvs {
                    [c.f32()?, c.f32()?]
                } else {
                    [half_to_f32(c.u16()?), half_to_f32(c.u16()?)]
                };
            }
            extra_uvs.extend_from_slice(&uvs[1..uv_set_count]);
            let heaviest = (0..4).max_by_key(|&i| weights[i]).unwrap_or(0);
            let chunk = chunks
                .iter()
                .rev()
                .find(|ch| ch.base_vertex as usize <= v)
                .ok_or_else(|| MeshError::Unsupported(format!("vertex {v} is in no chunk")))?;
            let bone = *chunk
                .bone_map
                .get(usize::from(bone_numbers[heaviest]))
                .ok_or_else(|| MeshError::Unsupported(format!("vertex {v}: bone number {}", bone_numbers[heaviest])))?;
            vertices.push(Vertex { position, normal, uv: uvs[0], bone });
        }

        if let Some(&bad) = indices.iter().find(|&&i| i as usize >= vertex_count) {
            return Err(MeshError::Unsupported(format!("index {bad} with {vertex_count} vertices")));
        }
        for s in &sections {
            let end = u64::from(s.base_index) + 3 * u64::from(s.triangle_count);
            if end > indices.len() as u64 || usize::from(s.chunk_index) >= chunks.len() {
                return Err(MeshError::Unsupported(format!("section {s:?} outside the index buffer or chunk list")));
            }
        }

        Ok(Self {
            bounds_origin,
            bounds_extent,
            bounds_radius,
            materials,
            bones,
            lod_count,
            sections,
            chunks,
            indices,
            vertices,
            uv_set_count,
            extra_uvs,
        })
    }

    /// The second texture-coordinate set of vertex `index` (zero when the mesh has only one).
    pub fn second_uv(&self, index: usize) -> [f32; 2] {
        if self.uv_set_count < 2 {
            return [0.0, 0.0];
        }
        self.extra_uvs.get(index * (self.uv_set_count - 1)).copied().unwrap_or([0.0, 0.0])
    }

    /// The smallest and largest corner of the box around the vertices that `indices` use.
    pub fn bounding_box(&self, indices: &[u32]) -> Option<([f32; 3], [f32; 3])> {
        let mut it = indices.iter().filter_map(|&i| self.vertices.get(i as usize));
        let first = it.next()?.position;
        Some(it.fold((first, first), |(mut lo, mut hi), v| {
            for a in 0..3 {
                lo[a] = lo[a].min(v.position[a]);
                hi[a] = hi[a].max(v.position[a]);
            }
            (lo, hi)
        }))
    }
}

/// A packed normal component: a byte mapping 0…255 onto −1…1.
fn unpack(b: u8) -> f32 {
    f32::from(b) / 127.5 - 1.0
}

/// An FName: name-table index plus number (a number above 0 appends `_<number-1>`).
fn read_name(c: &mut Cursor<'_>, names: &[String]) -> Result<String> {
    let (index, number) = (c.i32()?, c.i32()?);
    let base = usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i))
        .ok_or_else(|| MeshError::Unsupported(format!("name index {index}")))?;
    Ok(if number > 0 { format!("{base}_{}", number - 1) } else { base.clone() })
}
