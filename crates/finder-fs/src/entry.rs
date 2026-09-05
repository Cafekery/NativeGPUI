//! The record every consumer reads: one directory entry with what Finder shows for it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// The last path component.
    pub name: String,
    pub kind: EntryKind,
    /// Logical bytes; for a directory, what the index knows so far (0 until crawled).
    pub size: u64,
    pub modified: SystemTime,
    pub created: SystemTime,
    /// When the file landed in its folder (Finder's "Date Added"), if the volume records it.
    pub added: Option<SystemTime>,
    /// A dot-file or `UF_HIDDEN`: Finder hides it unless ⇧⌘. is on.
    pub hidden: bool,
    /// A directory Finder shows as a file: an app, a bundle, a document package.
    pub package: bool,
    /// The volume's file id (inode), stable across renames.
    pub file_id: u64,
}

impl Entry {
    /// A directory the user can navigate into — packages are files to the user.
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory && !self.package
    }

    /// The filename extension without the dot, as Finder would treat it (not for dot-files).
    pub fn extension(&self) -> Option<&str> {
        let (stem, ext) = self.name.rsplit_once('.')?;
        (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(&self.name)
    }
}

/// One directory read at one moment.
#[derive(Clone, Debug)]
pub struct Listing {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub read_at: SystemTime,
}
