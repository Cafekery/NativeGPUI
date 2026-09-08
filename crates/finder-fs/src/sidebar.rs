//! Finder's own sidebar lists: the Favorites the user keeps and the Locations Finder shows.
//!
//! Both are keyed archives of plain Foundation objects under
//! `~/Library/Application Support/com.apple.sharedfilelist`, one item per row, each holding a
//! bookmark and the properties Finder hangs on it.

use std::path::PathBuf;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSKeyedUnarchiver, NSNumber, NSString, NSURL,
    NSURLBookmarkResolutionOptions,
};

const LISTS: &str = "Library/Application Support/com.apple.sharedfilelist";

/// Newest first: macOS 26 writes `sfl4`, earlier releases `sfl3` and `sfl2`.
const SUFFIXES: [&str; 3] = ["sfl4", "sfl3", "sfl2"];

const FAVORITES: &str = "com.apple.LSSharedFileList.FavoriteItems";

const VOLUMES: &str = "com.apple.LSSharedFileList.FavoriteVolumes";

/// A Locations row that stands for something other than a mounted volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    Computer,
    ICloudDrive,
    Home,
    AirDrop,
    Network,
    Trash,
    RemoteDisc,
}

impl Special {
    fn parse(identifier: &str) -> Option<Self> {
        let last = identifier.rsplit('.').next()?;
        Some(match last {
            "IsComputer" => Self::Computer,
            "IsICloudDrive" => Self::ICloudDrive,
            "IsHome" => Self::Home,
            "IsMeetingRoom" | "IsAirDrop" => Self::AirDrop,
            "IsNetwork" | "IsBonjour" | "IsBonjourComputers" => Self::Network,
            "IsTrash" => Self::Trash,
            "IsRemoteDisc" => Self::RemoteDisc,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
/// One row of a list, in the order the user left it.
pub struct Item {
    /// Whether Finder draws the row: a row the user unticked stays in the list, hidden.
    pub visible: bool,
    pub path: Option<PathBuf>,
    /// The name Finder stores for the row, which a renamed or pathless row has and others do not.
    pub name: Option<String>,
    pub special: Option<Special>,
    pub volume: bool,
    /// The volume the system booted from.
    pub root: bool,
}

#[derive(Clone, Debug)]
/// The Locations list: its rows and the three classes of volume Finder is set to show.
pub struct Locations {
    pub items: Vec<Item>,
    pub hard_drives: bool,
    pub ejectable: bool,
    pub network: bool,
}

pub fn favorites() -> Option<Vec<Item>> {
    Some(read(FAVORITES)?.0)
}

pub fn locations() -> Option<Locations> {
    let (items, properties) = read(VOLUMES)?;
    let flag = |key: &str| {
        properties
            .as_ref()
            .and_then(|dict| number(dict, &format!("{VOLUMES}.{key}")))
            .is_none_or(|number| number.boolValue())
    };
    Some(Locations {
        items,
        hard_drives: flag("ShowHardDrives"),
        ejectable: flag("ShowEjectableVolumes"),
        network: flag("ShowNetworkVolumes"),
    })
}

type List = (Vec<Item>, Option<Retained<NSDictionary>>);

fn read(name: &str) -> Option<List> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let root = SUFFIXES.iter().find_map(|suffix| {
        let path = home.join(LISTS).join(format!("{name}.{suffix}"));
        decode(&std::fs::read(path).ok()?)
    })?;
    let items = root
        .objectForKey(&NSString::from_str("items"))
        .and_then(|items| items.downcast::<NSArray>().ok())
        .map(|items| items.iter().filter_map(|item| entry(&item)).collect())
        .unwrap_or_default();
    let properties = root
        .objectForKey(&NSString::from_str("properties"))
        .and_then(|properties| properties.downcast::<NSDictionary>().ok());
    Some((items, properties))
}

fn decode(bytes: &[u8]) -> Option<Retained<NSDictionary>> {
    let data = NSData::with_bytes(bytes);
    let unarchiver = unsafe {
        NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
    }
    .ok()?;
    // The lists archive only Foundation containers, so secure coding would reject nothing and
    // demanding it would fail every decode: the classes are not known ahead of time.
    unsafe { unarchiver.setRequiresSecureCoding(false) };
    let root = unsafe { unarchiver.decodeObjectForKey(&NSString::from_str("root")) }?;
    root.downcast::<NSDictionary>().ok()
}

fn entry(item: &AnyObject) -> Option<Item> {
    let item = item.downcast_ref::<NSDictionary>()?;
    let properties = item
        .objectForKey(&NSString::from_str("CustomItemProperties"))
        .and_then(|properties| properties.downcast::<NSDictionary>().ok());
    let property = |key: &str| {
        properties
            .as_ref()
            .and_then(|properties| properties.objectForKey(&NSString::from_str(key)))
    };
    let flag = |key: &str| {
        property(key)
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .is_some_and(|number| number.boolValue())
    };
    let special = property("com.apple.LSSharedFileList.SpecialItemIdentifier")
        .and_then(|value| value.downcast::<NSString>().ok())
        .and_then(|value| Special::parse(&value.to_string()));
    Some(Item {
        visible: number(item, "visibility").is_none_or(|number| number.integerValue() == 0),
        path: item
            .objectForKey(&NSString::from_str("Bookmark"))
            .and_then(|data| data.downcast::<NSData>().ok())
            .and_then(|data| resolve(&data)),
        name: item
            .objectForKey(&NSString::from_str("Name"))
            .and_then(|name| name.downcast::<NSString>().ok())
            .map(|name| name.to_string()),
        special,
        volume: flag("NSURLIsVolumeKey"),
        root: flag("NSURLVolumeIsRootFileSystemKey"),
    })
}

fn number(dict: &NSDictionary, key: &str) -> Option<Retained<NSNumber>> {
    dict.objectForKey(&NSString::from_str(key))?
        .downcast::<NSNumber>()
        .ok()
}

fn resolve(bookmark: &NSData) -> Option<PathBuf> {
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            bookmark,
            NSURLBookmarkResolutionOptions::WithoutUI
                | NSURLBookmarkResolutionOptions::WithoutMounting,
            None,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    Some(PathBuf::from(url.path()?.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_special_identifier_names_the_row_it_stands_for() {
        assert_eq!(
            Special::parse("com.apple.LSSharedFileList.IsHome"),
            Some(Special::Home)
        );
        assert_eq!(
            Special::parse("com.apple.LSSharedFileList.IsMeetingRoom"),
            Some(Special::AirDrop)
        );
        assert_eq!(
            Special::parse("com.apple.LSSharedFileList.IsAnything"),
            None
        );
    }

    #[test]
    fn this_mac_reads_its_own_lists() {
        let Some(favorites) = favorites() else {
            return;
        };
        assert!(
            favorites
                .iter()
                .all(|item| item.path.is_some() || item.name.is_some()),
            "every Favorites row names something"
        );
        let Some(locations) = locations() else {
            return;
        };
        assert!(
            locations
                .items
                .iter()
                .any(|item| item.special == Some(Special::Trash)),
            "Finder's Locations list always carries Trash"
        );
    }
}
