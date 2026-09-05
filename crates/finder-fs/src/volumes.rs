//! Mounted volumes and the standard folders, for the sidebar and the status bar.
//!
//! `mounted` lists what Finder's Locations shows, with the capacity the status bar reports;
//! `standard` gives the user's Desktop, Documents, Downloads, … as they exist.

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2_foundation::{
    NSArray, NSDictionary, NSFileManager, NSNumber, NSString, NSURL, NSURLResourceKey,
    NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeIsEjectableKey,
    NSURLVolumeIsLocalKey, NSURLVolumeIsRemovableKey, NSURLVolumeNameKey,
    NSURLVolumeTotalCapacityKey, NSVolumeEnumerationOptions,
};

#[derive(Clone, Debug)]
pub struct Volume {
    pub name: String,
    pub path: PathBuf,
    pub ejectable: bool,
    pub network: bool,
    pub capacity: u64,
    pub available: u64,
}

fn keys() -> Retained<NSArray<NSURLResourceKey>> {
    unsafe {
        NSArray::from_slice(&[
            NSURLVolumeNameKey,
            NSURLVolumeIsEjectableKey,
            NSURLVolumeIsRemovableKey,
            NSURLVolumeIsLocalKey,
            NSURLVolumeTotalCapacityKey,
            NSURLVolumeAvailableCapacityForImportantUsageKey,
        ])
    }
}

fn string_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> Option<String> {
    values
        .objectForKey(key)?
        .downcast::<NSString>()
        .ok()
        .map(|s| s.to_string())
}

fn bool_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> bool {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .is_some_and(|n| n.boolValue())
}

fn u64_value(
    values: &NSDictionary<NSURLResourceKey, objc2::runtime::AnyObject>,
    key: &NSString,
) -> u64 {
    values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSNumber>().ok())
        .map(|n| n.longLongValue().max(0) as u64)
        .unwrap_or(0)
}

fn volume_at(url: &NSURL, keys: &NSArray<NSURLResourceKey>) -> Option<Volume> {
    let values = url.resourceValuesForKeys_error(keys).ok()?;
    let name_key = unsafe { NSURLVolumeNameKey };
    Some(Volume {
        name: string_value(&values, name_key).unwrap_or_default(),
        path: url
            .path()
            .map(|p| PathBuf::from(p.to_string()))
            .unwrap_or_default(),
        ejectable: bool_value(&values, unsafe { NSURLVolumeIsEjectableKey })
            || bool_value(&values, unsafe { NSURLVolumeIsRemovableKey }),
        network: !bool_value(&values, unsafe { NSURLVolumeIsLocalKey }),
        capacity: u64_value(&values, unsafe { NSURLVolumeTotalCapacityKey }),
        available: u64_value(&values, unsafe {
            NSURLVolumeAvailableCapacityForImportantUsageKey
        }),
    })
}

pub fn mounted() -> Vec<Volume> {
    let fm = NSFileManager::defaultManager();
    let keys = keys();
    let Some(urls) = fm.mountedVolumeURLsIncludingResourceValuesForKeys_options(
        Some(&keys),
        NSVolumeEnumerationOptions::SkipHiddenVolumes,
    ) else {
        return Vec::new();
    };
    let mut volumes: Vec<Volume> = urls
        .to_vec()
        .iter()
        .filter_map(|url| volume_at(url, &keys))
        .collect();
    if let Some(boot) = volumes
        .iter()
        .position(|v| v.path.as_path() == std::path::Path::new("/"))
    {
        volumes.swap(0, boot);
    }
    volumes
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_volume_is_first_and_sized() {
        let volumes = mounted();
        assert!(!volumes.is_empty());
        assert_eq!(volumes[0].path, PathBuf::from("/"));
        assert!(volumes[0].capacity > 0);
    }

    #[test]
    fn standard_folders_are_under_home() {
        let home = standard(Standard::Home).unwrap();
        assert_eq!(standard(Standard::Desktop).unwrap(), home.join("Desktop"));
    }
}
