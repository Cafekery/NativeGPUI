//! The file system behind the Finder demo. Nothing here knows about GPUI or AppKit views: it
//! reads directories, keeps a whole-Mac index, watches for changes and performs file operations.

pub mod complete;
pub mod entry;
pub mod index;
pub mod kind;
pub mod list;
pub mod meta;
pub mod name;
pub mod ops;
pub mod queue;
pub mod sidebar;
pub mod sort;
pub mod tags;
pub mod volumes;
pub mod watch;

pub use entry::{Entry, EntryKind};
