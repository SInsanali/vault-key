//! "Gestalt" meshes: one skeletal mesh per weapon type that holds every part of every maker,
//! plus a definition that names each part's run of triangles.

use gamedata::package::UPackage;

use crate::error::{MeshError, Result};
use crate::skeletal::SkeletalMesh;

/// One named piece of a gestalt mesh: a run of triangles in its index buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The name part definitions use, such as `AR_Barrel_Dahl`.
    pub name: String,
    pub material_index: u32,
    /// Offset of the fragment's first index in [`SkeletalMesh::indices`].
    pub first_index: u32,
    pub triangle_count: u32,
}

/// A `GestaltSkeletalMeshDefinition` with its mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct GestaltMesh {
    /// Path of the definition, such as `Weap_AssaultRifles.GestaltDef_AssaultRifle`.
    pub definition_path: String,
    pub mesh: SkeletalMesh,
    pub fragments: Vec<Fragment>,
}

impl GestaltMesh {
    /// A plain skeletal mesh (export `index`) held as a gestalt mesh with one fragment,
    /// `name`, covering all of it. Class mods are drawn this way.
    pub fn whole(pkg: &UPackage, index: usize, name: &str) -> Result<Self> {
        let definition_path = pkg.export_path(index)?;
        let mesh = SkeletalMesh::from_export(pkg, index)?;
        let triangle_count = u32::try_from(mesh.indices.len() / 3)
            .map_err(|_| MeshError::Unsupported(format!("{definition_path}: too many triangles")))?;
        let fragments = vec![Fragment { name: name.to_string(), material_index: 0, first_index: 0, triangle_count }];
        Ok(Self { definition_path, mesh, fragments })
    }

    /// Reads the definition at export `index` of `pkg` and the mesh it points to, which must
    /// be in the same package.
    pub fn from_export(pkg: &UPackage, index: usize) -> Result<Self> {
        let definition_path = pkg.export_path(index)?;
        let class = pkg.class_name_of_export(index);
        if class != "GestaltSkeletalMeshDefinition" {
            return Err(MeshError::Unsupported(format!("{definition_path} is a {class}")));
        }
        let props = pkg.properties_of_export(index)?;
        let mesh_ref = props
            .object("GestaltSkeletalMesh")
            .filter(|&r| r > 0)
            .ok_or_else(|| MeshError::Missing(format!("{definition_path}: GestaltSkeletalMesh in this package")))?;
        let mesh = SkeletalMesh::from_export(pkg, (mesh_ref - 1) as usize)?;

        // `GestaltInfos` has one entry per LOD; only the first is used.
        let infos = props
            .get("GestaltInfos")
            .ok_or_else(|| MeshError::Missing(format!("{definition_path}: GestaltInfos")))?;
        let infos = pkg.struct_array(infos)?;
        let parts = infos
            .first()
            .and_then(|info| info.get("Parts"))
            .ok_or_else(|| MeshError::Missing(format!("{definition_path}: GestaltInfos[0].Parts")))?;
        let mut fragments = Vec::new();
        for part in pkg.struct_array(parts)? {
            let name = part
                .name("SkeletalMeshFragmentName")
                .ok_or_else(|| MeshError::Missing(format!("{definition_path}: a fragment name")))?
                .to_string();
            let number = |key: &str| -> Result<u32> {
                u32::try_from(part.int(key).unwrap_or(0))
                    .map_err(|_| MeshError::Unsupported(format!("{definition_path}: {name}.{key} is negative")))
            };
            let fragment = Fragment {
                material_index: number("MaterialIndex")?,
                first_index: number("FirstIndex")?,
                triangle_count: number("NumPrimitives")?,
                name,
            };
            let end = u64::from(fragment.first_index) + 3 * u64::from(fragment.triangle_count);
            if end > mesh.indices.len() as u64 {
                return Err(MeshError::Unsupported(format!(
                    "{definition_path}: fragment {} ends at index {end} of {}",
                    fragment.name,
                    mesh.indices.len()
                )));
            }
            fragments.push(fragment);
        }
        Ok(Self { definition_path, mesh, fragments })
    }

    /// The fragments with this name (names compare without regard to case, as engine names
    /// do). Usually one; the launcher mesh has five runs called `Acc_Barrel_Elemental2`.
    pub fn fragments_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Fragment> {
        self.fragments.iter().filter(move |f| f.name.eq_ignore_ascii_case(name))
    }

    /// The vertex indices of a fragment's triangles (empty for a fragment that is not this
    /// mesh's).
    pub fn indices_of(&self, fragment: &Fragment) -> &[u32] {
        let start = fragment.first_index as usize;
        let end = start.saturating_add(3usize.saturating_mul(fragment.triangle_count as usize));
        self.mesh.indices.get(start..end).unwrap_or(&[])
    }
}
