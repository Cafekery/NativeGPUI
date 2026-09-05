//! Mounted volumes and the standard folders, for the sidebar and the status bar.
//!
//! Contract (stub): `mounted` lists what Finder's Locations shows, with the capacity the status
//! bar reports; `standard` gives the user's Desktop, Documents, Downloads, … as they exist.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub ejectable: bool,
    pub network: bool,
    pub capacity: u64,
    pub available: u64,
}

pub fn mounted() -> Vec<Volume> {
    Vec::new()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Home,
    Desktop,
    Documents,
    Downloads,
    Applications,
    Movies,
    Music,
    Pictures,
    Trash,
}

pub fn standard(which: Standard) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(match which {
        Standard::Home => home,
        Standard::Desktop => home.join("Desktop"),
        Standard::Documents => home.join("Documents"),
        Standard::Downloads => home.join("Downloads"),
        Standard::Applications => PathBuf::from("/Applications"),
        Standard::Movies => home.join("Movies"),
        Standard::Music => home.join("Music"),
        Standard::Pictures => home.join("Pictures"),
        Standard::Trash => home.join(".Trash"),
    })
}
