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
    pub size: u64,
    pub modified: SystemTime,
    pub created: SystemTime,
    pub added: Option<SystemTime>,
    pub hidden: bool,
    pub package: bool,
    pub ext_hidden: bool,
    /// FinderInfo's alias flag.
    pub alias: bool,
    pub custom_icon: bool,
    pub dataless: bool,
    pub readable: bool,
    pub file_id: u64,
    pub children: Option<u32>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Directory && !self.package
    }

    pub fn extension(&self) -> Option<&str> {
        let (stem, ext) = self.name.rsplit_once('.')?;
        (!stem.is_empty() && !ext.is_empty() && !ext.contains(' ')).then_some(ext)
    }

    pub fn path_in(&self, dir: &Path) -> PathBuf {
        dir.join(&self.name)
    }
}
