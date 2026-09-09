use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

use finder_fs::volumes;
use objc2_foundation::{NSFileManager, NSString};

use super::paths::intern;

struct Mounted {
    mount: String,
    name: &'static str,
    available: u64,
    capacity: u64,
    format: &'static str,
    network: bool,
}

fn volumes_seen() -> &'static RwLock<Vec<Mounted>> {
    static SEEN: OnceLock<RwLock<Vec<Mounted>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
}

/// Enumerating the mounted volumes costs milliseconds, so no frame may call this.
pub(crate) fn refresh_volumes() {
    let mounted = volumes::mounted()
        .iter()
        .map(|volume| Mounted {
            mount: volume.path.to_string_lossy().into_owned(),
            name: intern(&volume.name),
            available: volume.available,
            capacity: volume.capacity,
            format: intern(&volume.format),
            network: volume.network,
        })
        .collect();
    *volumes_seen().write().unwrap() = mounted;
}

fn boot_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = NSFileManager::defaultManager()
            .displayNameAtPath(&NSString::from_str("/"))
            .to_string();
        intern(&name)
    })
}

pub(super) fn volume_of(path: &str) -> (&'static str, &'static str) {
    let under =
        |mount: &str| mount == "/" || path == mount || path.starts_with(&format!("{mount}/"));
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .filter(|volume| under(&volume.mount))
        .max_by_key(|volume| volume.mount.len())
        .map(|volume| (intern(&volume.mount), volume.name))
        .unwrap_or(("/", boot_name()))
}

pub(crate) fn volume_roots() -> Vec<&'static str> {
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .map(|volume| intern(&volume.mount))
        .collect()
}

pub(crate) fn network_roots() -> Vec<&'static str> {
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .filter(|volume| volume.network)
        .map(|volume| intern(&volume.mount))
        .collect()
}

pub(crate) fn available_bytes(id: &str) -> u64 {
    let (mount, _) = volume_of(id);
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .find(|volume| volume.mount == mount)
        .map_or(0, |volume| volume.available)
}

pub(crate) struct VolumeInfo {
    pub(crate) name: &'static str,
    pub(crate) capacity: u64,
    pub(crate) available: u64,
    pub(crate) format: &'static str,
}

pub(crate) fn volume_at(id: &str) -> Option<VolumeInfo> {
    volumes_seen()
        .read()
        .unwrap()
        .iter()
        .find(|volume| volume.mount == id)
        .map(|volume| VolumeInfo {
            name: volume.name,
            capacity: volume.capacity,
            available: volume.available,
            format: volume.format,
        })
}

/// The folders Finder draws with their own artwork.
pub(super) fn special() -> &'static HashSet<String> {
    static PLACES: OnceLock<HashSet<String>> = OnceLock::new();
    PLACES.get_or_init(|| {
        use finder_fs::volumes::Standard::*;
        let mut places: HashSet<String> = [
            Home,
            Desktop,
            Documents,
            Downloads,
            Applications,
            Movies,
            Music,
            Pictures,
            Trash,
        ]
        .into_iter()
        .filter_map(volumes::standard)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
        let home = volumes::standard(Home).unwrap_or_default();
        for leaf in ["Public", "Library", "Developer", "Sites", "Applications"] {
            places.insert(home.join(leaf).to_string_lossy().into_owned());
        }
        for path in [
            "/",
            "/Applications/Utilities",
            "/Library",
            "/System",
            "/System/Library",
            "/Users",
            "/Users/Shared",
            "/Volumes",
        ] {
            places.insert(path.to_string());
        }
        places
    })
}
