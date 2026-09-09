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
    pub name: String,
    pub kind: EntryKind,
    /// For a directory, what the index knows so far: 0 until crawled.
    pub size: u64,
    pub modified: SystemTime,
    pub created: SystemTime,
    /// Finder's "Date Added", where the volume records it.
    pub added: Option<SystemTime>,
    /// A dot-file or `UF_HIDDEN`: Finder hides it unless ⇧⌘. is on.
    pub hidden: bool,
    /// A directory Finder shows as a file: an app, a bundle, a document package.
    pub package: bool,
    /// Evicted from this Mac: iCloud holds the data and Finder badges the row with a cloud.
    pub dataless: bool,
    /// The volume's inode, stable across renames; 0 for an entry the index answered from memory.
    pub file_id: u64,
    /// Hidden ones included, where the volume counts them.
    pub children: Option<u32>,
}

impl Entry {
    /// A package is a file to the user, not a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory && !self.package
    }

    /// As Finder treats it: no extension for a dot-file.
    pub fn extension(&self) -> Option<&str> {
        let (stem, ext) = self.name.rsplit_once('.')?;
        (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct Listing {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub read_at: SystemTime,
}
