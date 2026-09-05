//! Finder tags: the user's named colours from Finder's preferences, and the tags on a file.
//!
//! Contract (stub): `all` reads Finder's tag list (name + colour index) so the demo shows the
//! user's own tags; `of`/`set` read and write a file's tags through the URL resource key
//! Finder uses, so both apps agree.

use std::io;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Finder's label colour index: 0 none, 1 grey, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange.
    pub color: u8,
}

pub fn all() -> Vec<Tag> {
    Vec::new()
}

pub fn of(_path: &Path) -> Vec<Tag> {
    Vec::new()
}

pub fn set(_path: &Path, _tags: &[Tag]) -> io::Result<()> {
    Ok(())
}
