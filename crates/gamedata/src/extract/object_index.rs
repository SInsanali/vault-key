use std::collections::HashMap;

use crate::package::UPackage;

/// An export: the index of its package in the list being built from, and its export index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Loc {
    pub(crate) package: usize,
    pub(crate) export: usize,
}

/// Locates exports by full object path across several packages. The first package wins when
/// two define the same path.
pub(crate) struct ObjectIndex<'a> {
    by_path: HashMap<&'a str, Loc>,
}

impl<'a> ObjectIndex<'a> {
    pub(crate) fn new(packages: &'a [UPackage]) -> Self {
        let mut by_path = HashMap::with_capacity(packages.iter().map(|p| p.exports.len()).sum());
        for (package, p) in packages.iter().enumerate() {
            for (path, export) in p.export_paths() {
                by_path.entry(path).or_insert(Loc { package, export });
            }
        }
        Self { by_path }
    }

    pub(crate) fn locate(&self, path: &str) -> Option<Loc> {
        self.by_path.get(path).copied()
    }
}
