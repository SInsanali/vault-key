//! UE3 package reading: the compression wrapper, the tables, and tagged properties.

mod file;
mod properties;
pub(crate) mod reader;
mod upackage;

pub use file::{decompress, read_package, PACKAGE_TAG};
pub use properties::{Property, PropertyList, PropertyValue};
pub use upackage::{ExportEntry, ImportEntry, UPackage, SUPPORTED_LICENSEE, SUPPORTED_VERSION};
