use std::path::PathBuf;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CFURLPathStyle,
};
use objc2_core_services::{
    LSSharedFileList, LSSharedFileListItem, kLSSharedFileListDoNotMountVolumes,
    kLSSharedFileListNoUserInteraction,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSKeyedUnarchiver, NSNumber, NSString, NSURL, NSURLPathKey,
    ns_string,
};

use crate::tags::Tag;
use crate::volumes::{self, Standard, Volume};
use crate::{icloud, sort};

const LISTS: &str = "Library/Application Support/com.apple.sharedfilelist";
const SUFFIXES: [&str; 3] = ["sfl4", "sfl3", "sfl2"];
const FAVORITES: &str = "com.apple.LSSharedFileList.FavoriteItems";
const VOLUMES: &str = "com.apple.LSSharedFileList.FavoriteVolumes";
const HIDDEN: &str = "com.apple.LSSharedFileList.ItemIsHidden";
const SHOWN: [&str; 3] = [
    "com.apple.LSSharedFileList.FavoriteVolumes.ShowHardDrives",
    "com.apple.LSSharedFileList.FavoriteVolumes.ShowEjectableVolumes",
    "com.apple.LSSharedFileList.FavoriteVolumes.ShowNetworkVolumes",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    Computer,
    ICloudDrive,
    FileProvider,
    Home,
    AirDrop,
    Network,
    Trash,
    RemoteDisc,
}

impl Special {
    fn parse(identifier: &str) -> Option<Self> {
        Some(match identifier.rsplit('.').next()? {
            "IsComputer" => Self::Computer,
            "IsICloudDrive" => Self::ICloudDrive,
            "FileProvider" => Self::FileProvider,
            "IsHome" => Self::Home,
            "IsMeetingRoom" | "IsAirDrop" => Self::AirDrop,
            "IsNetwork" | "IsBonjour" | "IsBonjourComputers" => Self::Network,
            "IsTrash" => Self::Trash,
            "IsRemoteDisc" => Self::RemoteDisc,
            _ => return None,
        })
    }
}

struct Item {
    visible: bool,
    path: Option<PathBuf>,
    name: Option<String>,
    special: Option<Special>,
}

impl Item {
    fn listed(&self) -> Option<Listed> {
        match &self.name {
            Some(name) => Some(Listed::new(name.clone(), self.path.clone(), self.special)),
            None => Listed::at(self.path.clone()?, self.special),
        }
    }
}

/// A sidebar row as Finder shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub title: String,
    pub path: Option<PathBuf>,
    pub special: Option<Special>,
    pub network: bool,
    pub ejectable: bool,
    pub server: Option<String>,
}

impl Listed {
    fn new(title: impl Into<String>, path: Option<PathBuf>, special: Option<Special>) -> Self {
        Listed {
            title: title.into(),
            path,
            special,
            network: false,
            ejectable: false,
            server: None,
        }
    }

    fn at(path: PathBuf, special: Option<Special>) -> Option<Self> {
        let title = path.file_name()?.to_string_lossy().into_owned();
        Some(Listed::new(title, Some(path), special))
    }

    fn volume(title: String, volume: &Volume) -> Self {
        Listed {
            network: volume.network,
            ejectable: volume.ejectable,
            server: volume.server.clone(),
            ..Listed::new(title, Some(volume.path.clone()), None)
        }
    }
}

/// Finder lists home under Locations, never under Favorites.
pub fn listed_favorites() -> Vec<Listed> {
    let Some((items, _)) = read(FAVORITES) else {
        use Standard::*;
        return [Applications, Desktop, Documents, Downloads]
            .into_iter()
            .filter_map(volumes::standard)
            .filter_map(|path| Listed::at(path, None))
            .collect();
    };
    items
        .iter()
        .filter(|item| item.visible && item.path.is_some() && item.special != Some(Special::Home))
        .filter_map(Item::listed)
        .collect()
}

pub fn listed_locations() -> Vec<Listed> {
    let (items, shown) = read(VOLUMES).unwrap_or_else(|| {
        let item = |special| Item {
            visible: true,
            path: None,
            name: None,
            special: Some(special),
        };
        let items = vec![
            item(Special::Home),
            item(Special::AirDrop),
            item(Special::Trash),
        ];
        (items, [true; 3])
    });
    arrange(&items, shown, &volumes::mounted())
}

/// Finder's Hard disks, External disks and Connected servers sidebar settings.
pub fn volume_kinds_shown() -> [bool; 3] {
    read(VOLUMES).map_or([true; 3], |(_, shown)| shown)
}

/// Finder's sections in its own order, whatever order its list keeps.
fn arrange(
    items: &[Item],
    [disks, external, network]: [bool; 3],
    mounted: &[Volume],
) -> Vec<Listed> {
    let listed = |special| {
        items
            .iter()
            .filter(move |item| item.visible && item.special == Some(special))
    };
    let has = |special| listed(special).next().is_some();
    let shown = |volume: &&Volume| {
        !items.iter().any(|item| {
            // The special rows' bookmarks point at `/` too.
            !item.visible
                && item.special.is_none()
                && (item.path.as_deref() == Some(volume.path.as_path())
                    || item.path.is_none() && item.name.as_deref() == Some(volume.name.as_str()))
        }) && match (volume.network, volume.ejectable) {
            (true, _) => network,
            (_, true) => external,
            _ => disks,
        }
    };
    let mut rows = Vec::new();
    if has(Special::ICloudDrive) {
        let drive = icloud::drive();
        rows.push(Listed::new(
            "iCloud Drive",
            drive,
            Some(Special::ICloudDrive),
        ));
    }
    rows.extend(listed(Special::FileProvider).filter_map(Item::listed));
    if has(Special::Home) {
        let home = volumes::standard(Standard::Home);
        rows.extend(home.and_then(|home| Listed::at(home, Some(Special::Home))));
    }
    for eject in [false, true] {
        rows.extend(
            mounted
                .iter()
                .filter(|volume| !volume.network && volume.ejectable == eject)
                .filter(shown)
                .map(|volume| Listed::volume(volume.name.clone(), volume)),
        );
    }
    if has(Special::AirDrop) {
        rows.push(Listed::new("AirDrop", None, Some(Special::AirDrop)));
    }
    let mut servers: Vec<Listed> = Vec::new();
    for volume in mounted.iter().filter(|volume| volume.network).filter(shown) {
        let title = crate::servers::title(volume.server.as_deref().unwrap_or(&volume.name));
        if !servers.iter().any(|row| row.title == title) {
            servers.push(Listed::volume(title, volume));
        }
    }
    servers.sort_by(|a, b| sort::name_cmp(&a.title, &b.title));
    rows.extend(servers);
    rows.push(Listed::new("Network", None, Some(Special::Network)));
    if has(Special::Trash) {
        let trash = volumes::standard(Standard::Trash);
        rows.push(Listed::new("Trash", trash, Some(Special::Trash)));
    }
    rows
}

fn read(name: &'static str) -> Option<(Vec<Item>, [bool; 3])> {
    let lists = crate::home()?.join(LISTS);
    let Some(root) = SUFFIXES
        .iter()
        .find_map(|suffix| decode(std::fs::read(lists.join(format!("{name}.{suffix}"))).ok()?))
    else {
        return shared(name);
    };
    let items = root
        .objectForKey(ns_string!("items"))
        .and_then(|items| items.downcast::<NSArray>().ok())
        .map(|items| items.iter().filter_map(|item| entry(&item)).collect())
        .unwrap_or_default();
    let properties = root
        .objectForKey(ns_string!("properties"))
        .and_then(|properties| properties.downcast::<NSDictionary>().ok());
    let shown = SHOWN.map(|key| {
        properties
            .as_deref()
            .and_then(|properties| number(properties, &NSString::from_str(key)))
            .is_none_or(|shown| shown.boolValue())
    });
    Some((items, shown))
}

/// An app may not read Finder's lists itself; `sharedfilelistd` hands over their visible items.
#[allow(deprecated)]
fn shared(name: &'static str) -> Option<(Vec<Item>, [bool; 3])> {
    let (list, items) = snapshot(name)?;
    let special = CFString::from_static_str("com.apple.LSSharedFileList.SpecialItemIdentifier");
    let items = items
        .iter()
        .map(|item| {
            let name = unsafe { item.display_name() }.to_string();
            Item {
                visible: true,
                path: resolved(&item).map(PathBuf::from),
                name: (!name.is_empty()).then_some(name),
                special: unsafe { item.property(&special) }
                    .and_then(|value| value.downcast::<CFString>().ok())
                    .and_then(|value| Special::parse(&value.to_string())),
            }
        })
        .collect();
    let shown = SHOWN.map(|key| {
        unsafe { list.property(&CFString::from_static_str(key)) }
            .and_then(|value| truthy(&value))
            .unwrap_or(true)
    });
    Some((items, shown))
}

#[allow(deprecated)]
fn snapshot(
    name: &'static str,
) -> Option<(
    CFRetained<LSSharedFileList>,
    CFRetained<CFArray<LSSharedFileListItem>>,
)> {
    let list = unsafe { LSSharedFileList::new(None, &CFString::from_static_str(name), None) }?;
    let items = unsafe { list.snapshot(std::ptr::null_mut()) }?;
    Some((list, unsafe { CFRetained::cast_unchecked(items) }))
}

#[allow(deprecated)]
fn resolved(item: &LSSharedFileListItem) -> Option<String> {
    let quiet = kLSSharedFileListNoUserInteraction | kLSSharedFileListDoNotMountVolumes;
    let url = unsafe { item.resolved_url(quiet, std::ptr::null_mut()) }?;
    Some(
        url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)?
            .to_string(),
    )
}

fn truthy(value: &CFType) -> Option<bool> {
    match value.downcast_ref::<CFBoolean>() {
        Some(value) => Some(value.as_bool()),
        None => Some(value.downcast_ref::<CFNumber>()?.as_i64()? != 0),
    }
}

/// The tags Finder's sidebar lists, in its order and colours, each with whether it is pinned there.
#[allow(deprecated)]
pub(crate) fn tags() -> Option<Vec<(Tag, bool)>> {
    let (_list, items) = snapshot("com.apple.LSSharedFileList.ProjectsItems")?;
    let hidden = CFString::from_static_str(HIDDEN);
    let pinned = CFString::from_static_str("kLSSharedTagFileListItemPinned");
    let label = CFString::from_static_str("kLSSharedTagFileListItemLabel");
    Some(
        items
            .iter()
            .filter(|item| {
                unsafe { item.property(&hidden) }.and_then(|value| truthy(&value)) != Some(true)
            })
            .map(|item| {
                let pinned = unsafe { item.property(&pinned) }
                    .is_none_or(|value| truthy(&value) == Some(true));
                let color = unsafe { item.property(&label) }
                    .and_then(|value| value.downcast_ref::<CFNumber>()?.as_i64())
                    .and_then(|color| u8::try_from(color).ok())
                    .unwrap_or_default();
                let name = unsafe { item.display_name() }.to_string();
                (Tag { name, color }, pinned)
            })
            .filter(|(tag, _)| !tag.name.is_empty())
            .collect(),
    )
}

/// Whether Finder's sidebar shows Recents and Shared, the rows above Favorites.
#[allow(deprecated)]
pub fn top_rows_shown() -> [bool; 2] {
    let hidden = CFString::from_static_str(HIDDEN);
    let paths: Vec<String> = snapshot("com.apple.LSSharedFileList.TopSidebarSection")
        .map(|(_list, items)| {
            items
                .iter()
                .filter(|item| {
                    unsafe { item.property(&hidden) }
                        .is_none_or(|value| truthy(&value) == Some(false))
                })
                .filter_map(|item| resolved(&item))
                .collect()
        })
        .unwrap_or_default();
    ["myDocuments.cannedSearch", "SharedDocuments.cannedSearch"]
        .map(|leaf| paths.iter().any(|path| path.ends_with(leaf)))
}

fn decode(bytes: Vec<u8>) -> Option<Retained<NSDictionary>> {
    let data = NSData::from_vec(bytes);
    let unarchiver = unsafe {
        NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
    }
    .ok()?;
    // The archived classes are not known ahead of time: secure coding would fail every decode.
    unsafe { unarchiver.setRequiresSecureCoding(false) };
    let root = unsafe { unarchiver.decodeObjectForKey(ns_string!("root")) }?;
    root.downcast::<NSDictionary>().ok()
}

fn entry(item: &AnyObject) -> Option<Item> {
    let item = item.downcast_ref::<NSDictionary>()?;
    let properties = item
        .objectForKey(ns_string!("CustomItemProperties"))
        .and_then(|properties| properties.downcast::<NSDictionary>().ok());
    let property = |key: &NSString| properties.as_ref()?.objectForKey(key);
    let special = property(ns_string!(
        "com.apple.LSSharedFileList.SpecialItemIdentifier"
    ))
    .and_then(|value| Special::parse(&value.downcast::<NSString>().ok()?.to_string()))
    .or_else(|| {
        let provider = property(ns_string!(
            "com.apple.finder.kLSSharedFileListItemIsFileProvider"
        ))?;
        let provider = provider.downcast::<NSNumber>().ok()?.boolValue();
        provider.then_some(Special::FileProvider)
    });
    Some(Item {
        visible: number(item, ns_string!("visibility"))
            .is_none_or(|visibility| visibility.integerValue() == 0),
        path: item
            .objectForKey(ns_string!("Bookmark"))
            .and_then(|bookmark| bookmark.downcast::<NSData>().ok())
            .and_then(|bookmark| stored_path(&bookmark)),
        name: item
            .objectForKey(ns_string!("Name"))
            .and_then(|name| name.downcast::<NSString>().ok())
            .map(|name| name.to_string()),
        special,
    })
}

fn number(dict: &NSDictionary, key: &NSString) -> Option<Retained<NSNumber>> {
    dict.objectForKey(key)?.downcast::<NSNumber>().ok()
}

/// Resolving a bookmark stats its target, which waits for as long as a dead network share does.
fn stored_path(bookmark: &NSData) -> Option<PathBuf> {
    let key = unsafe { NSURLPathKey };
    let values =
        NSURL::resourceValuesForKeys_fromBookmarkData(&NSArray::from_slice(&[key]), bookmark)?;
    let path = values.objectForKey(key)?.downcast::<NSString>().ok()?;
    Some(PathBuf::from(path.to_string()))
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
        assert_eq!(Special::parse("FileProvider"), Some(Special::FileProvider));
        assert_eq!(
            Special::parse("com.apple.LSSharedFileList.IsAnything"),
            None
        );
    }

    fn item(
        special: Option<Special>,
        name: Option<&str>,
        path: Option<&str>,
        visible: bool,
    ) -> Item {
        Item {
            visible,
            path: path.map(PathBuf::from),
            name: name.map(str::to_owned),
            special,
        }
    }

    fn volume(name: &str, path: &str, ejectable: bool, server: Option<&str>) -> Volume {
        Volume {
            name: name.into(),
            path: path.into(),
            ejectable: ejectable || server.is_some(),
            network: server.is_some(),
            capacity: 0,
            available: 0,
            format: String::new(),
            server: server.map(str::to_owned),
        }
    }

    #[test]
    fn locations_keep_finders_sections_whatever_order_its_list_keeps() {
        use Special::*;
        let items = [
            item(Some(Computer), None, Some("/"), false),
            item(Some(ICloudDrive), None, Some("/"), false),
            item(
                Some(FileProvider),
                Some("Noah Bucket"),
                Some("/cloud/S3Mount-NoahBucket"),
                true,
            ),
            item(Some(RemoteDisc), None, Some("/"), true),
            item(Some(ICloudDrive), Some("iCloud"), Some("/"), true),
            item(Some(Home), None, Some("/Users/jace"), true),
            item(Some(AirDrop), None, Some("/"), true),
            item(Some(Trash), Some("Trash"), Some("/"), true),
            item(None, Some("Gone"), Some("/Volumes/Gone"), false),
        ];
        let bonjour = "NG%20Jace%E2%80%99s%20Mini._smb._tcp.local";
        let mounted = [
            volume("Macintosh HD", "/", false, None),
            volume("share", "/Volumes/share", false, Some(bonjour)),
            volume("Storage", "/Volumes/Storage", false, Some("100.109.9.62")),
            volume("Queue", "/Volumes/Queue", true, None),
            volume("Photos", "/Volumes/Photos", false, Some("nas.local")),
            volume("Media", "/Volumes/Media", false, Some("100.109.9.62")),
            volume("Backup", "/Volumes/Backup", false, Some("10.0.0.9")),
            volume("Gone", "/Volumes/Gone", true, None),
        ];
        let rows = arrange(&items, [true; 3], &mounted);
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        let home = volumes::standard(Standard::Home).unwrap();
        let home = home.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            titles,
            [
                "iCloud Drive",
                "Noah Bucket",
                home,
                "Macintosh HD",
                "Queue",
                "AirDrop",
                "10.0.0.9",
                "100.109.9.62",
                "nas.local",
                "NG Jace’s Mini",
                "Network",
                "Trash"
            ]
        );
        let ejects: Vec<&str> = rows
            .iter()
            .filter(|row| row.ejectable)
            .map(|row| row.title.as_str())
            .collect();
        assert_eq!(
            ejects,
            [
                "Queue",
                "10.0.0.9",
                "100.109.9.62",
                "nas.local",
                "NG Jace’s Mini"
            ]
        );
        assert_eq!(
            rows[7].path.as_deref(),
            Some(PathBuf::from("/Volumes/Storage").as_path())
        );
        assert_eq!(rows[9].server.as_deref(), Some(bonjour));

        let titles: Vec<String> = arrange(&items, [false; 3], &mounted)
            .into_iter()
            .map(|row| row.title)
            .collect();
        assert!(
            !titles
                .iter()
                .any(|title| title == "Macintosh HD" || title == "100.109.9.62")
        );
    }
}
