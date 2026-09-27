use std::path::PathBuf;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFURLPathStyle};
use objc2_core_services::{
    LSSharedFileList, LSSharedFileListItem, kLSSharedFileListDoNotMountVolumes,
    kLSSharedFileListNoUserInteraction,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSKeyedUnarchiver, NSNumber, NSString, NSURL, NSURLPathKey,
};

use crate::volumes::{self, Standard, Volume};
use crate::{icloud, servers, sort};

const LISTS: &str = "Library/Application Support/com.apple.sharedfilelist";

const SUFFIXES: [&str; 3] = ["sfl4", "sfl3", "sfl2"];

const FAVORITES: &str = "com.apple.LSSharedFileList.FavoriteItems";

const VOLUMES: &str = "com.apple.LSSharedFileList.FavoriteVolumes";

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
        let last = identifier.rsplit('.').next()?;
        Some(match last {
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

#[derive(Clone, Debug)]
struct Item {
    visible: bool,
    path: Option<PathBuf>,
    name: Option<String>,
    special: Option<Special>,
}

impl Item {
    fn title(&self) -> Option<String> {
        let path = self.path.as_ref();
        self.name
            .clone()
            .or_else(|| Some(path?.file_name()?.to_string_lossy().into_owned()))
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
    /// The host a server row's volume was mounted from.
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
            .filter_map(|path| {
                let title = path.file_name()?.to_string_lossy().into_owned();
                Some(Listed::new(title, Some(path), None))
            })
            .collect();
    };
    items
        .iter()
        .filter(|item| item.visible && item.path.is_some() && item.special != Some(Special::Home))
        .filter_map(|item| Some(Listed::new(item.title()?, item.path.clone(), item.special)))
        .collect()
}

/// Nothing here waits on a share that stopped answering, which Finder lists all the same.
pub fn listed_locations() -> Vec<Listed> {
    let (items, properties) = read(VOLUMES).unwrap_or_else(|| {
        let item = |special| Item {
            visible: true,
            path: None,
            name: None,
            special: Some(special),
        };
        (
            vec![
                item(Special::Home),
                item(Special::AirDrop),
                item(Special::Trash),
            ],
            None,
        )
    });
    let shows = |key: &str| {
        properties
            .as_ref()
            .and_then(|dict| number(dict, &format!("{VOLUMES}.{key}")))
            .is_none_or(|number| number.boolValue())
    };
    arrange(
        &items,
        [
            shows("ShowHardDrives"),
            shows("ShowEjectableVolumes"),
            shows("ShowNetworkVolumes"),
        ],
        &volumes::mounted(),
    )
}

/// Finder's sections in its own order, whatever order its list keeps.
fn arrange(
    items: &[Item],
    [disks, ejectable, servers]: [bool; 3],
    mounted: &[Volume],
) -> Vec<Listed> {
    let listed = |special| {
        items
            .iter()
            .filter(move |item| item.visible && item.special == Some(special))
    };
    let shown = |volume: &&Volume| {
        !items.iter().any(|item| {
            // The special rows' bookmarks point at `/` too.
            !item.visible
                && item.special.is_none()
                && (item.path.as_deref() == Some(volume.path.as_path())
                    || item.path.is_none() && item.name.as_deref() == Some(volume.name.as_str()))
        }) && match (volume.network, volume.ejectable) {
            (true, _) => servers,
            (_, true) => ejectable,
            _ => disks,
        }
    };
    let mut rows = Vec::new();
    if listed(Special::ICloudDrive).next().is_some() {
        rows.push(Listed::new(
            "iCloud Drive",
            icloud::drive(),
            Some(Special::ICloudDrive),
        ));
    }
    rows.extend(listed(Special::FileProvider).filter_map(|item| {
        Some(Listed::new(
            item.title()?,
            item.path.clone(),
            Some(Special::FileProvider),
        ))
    }));
    let home = volumes::standard(Standard::Home).filter(|_| listed(Special::Home).next().is_some());
    if let Some(title) = home.as_ref().and_then(|home| home.file_name()) {
        let title = title.to_string_lossy();
        rows.push(Listed::new(title, home.clone(), Some(Special::Home)));
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
    if listed(Special::AirDrop).next().is_some() {
        rows.push(Listed::new("AirDrop", None, Some(Special::AirDrop)));
    }
    let mut servers: Vec<Listed> = Vec::new();
    for volume in mounted.iter().filter(|volume| volume.network).filter(shown) {
        let title = servers::title(volume.server.as_deref().unwrap_or(&volume.name));
        if !servers.iter().any(|row| row.title == title) {
            servers.push(Listed::volume(title, volume));
        }
    }
    servers.sort_by(|a, b| sort::name_cmp(&a.title, &b.title));
    rows.extend(servers);
    rows.push(Listed::new("Network", None, Some(Special::Network)));
    if listed(Special::Trash).next().is_some() {
        let trash = volumes::standard(Standard::Trash);
        rows.push(Listed::new("Trash", trash, Some(Special::Trash)));
    }
    rows
}

fn read(name: &str) -> Option<(Vec<Item>, Option<Retained<NSDictionary>>)> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let Some(root) = SUFFIXES.iter().find_map(|suffix| {
        let path = home.join(LISTS).join(format!("{name}.{suffix}"));
        decode(&std::fs::read(path).ok()?)
    }) else {
        return shared(name);
    };
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

/// An app may not read Finder's lists itself; `sharedfilelistd` hands over their visible items,
/// each resolved to where it now is.
#[allow(deprecated)]
fn shared(name: &str) -> Option<(Vec<Item>, Option<Retained<NSDictionary>>)> {
    let kind = match name {
        FAVORITES => unsafe { objc2_core_services::kLSSharedFileListFavoriteItems },
        _ => unsafe { objc2_core_services::kLSSharedFileListFavoriteVolumes },
    };
    let list = unsafe { LSSharedFileList::new(None, kind, None) }?;
    let snapshot = unsafe { list.snapshot(std::ptr::null_mut()) }?;
    let snapshot: CFRetained<CFArray<LSSharedFileListItem>> =
        unsafe { CFRetained::cast_unchecked(snapshot) };
    let special = CFString::from_static_str("com.apple.LSSharedFileList.SpecialItemIdentifier");
    let quiet = kLSSharedFileListNoUserInteraction | kLSSharedFileListDoNotMountVolumes;
    let items = snapshot
        .iter()
        .map(|item| {
            let name = unsafe { item.display_name() }.to_string();
            Item {
                visible: true,
                path: unsafe { item.resolved_url(quiet, std::ptr::null_mut()) }
                    .and_then(|url| url.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle))
                    .map(|path| PathBuf::from(path.to_string())),
                name: (!name.is_empty()).then_some(name),
                special: unsafe { item.property(&special) }
                    .and_then(|value| value.downcast::<CFString>().ok())
                    .and_then(|value| Special::parse(&value.to_string())),
            }
        })
        .collect();
    let (keys, values): (Vec<_>, Vec<_>) = [
        "ShowHardDrives",
        "ShowEjectableVolumes",
        "ShowNetworkVolumes",
    ]
    .into_iter()
    .filter_map(|key| {
        let key = format!("{VOLUMES}.{key}");
        let value = unsafe { list.property(&CFString::from_str(&key)) }?;
        let on = match value.downcast_ref::<CFBoolean>() {
            Some(on) => on.as_bool(),
            None => value.downcast_ref::<CFNumber>()?.as_i64()? != 0,
        };
        Some((NSString::from_str(&key), NSNumber::new_bool(on)))
    })
    .unzip();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let properties = NSDictionary::from_retained_objects(&keys, &values);
    Some((items, Some(unsafe { Retained::cast_unchecked(properties) })))
}

fn decode(bytes: &[u8]) -> Option<Retained<NSDictionary>> {
    let data = NSData::with_bytes(bytes);
    let unarchiver = unsafe {
        NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
    }
    .ok()?;
    // The archived classes are not known ahead of time: secure coding would fail every decode.
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
    let file_provider = property("com.apple.finder.kLSSharedFileListItemIsFileProvider")
        .and_then(|value| value.downcast::<NSNumber>().ok())
        .is_some_and(|number| number.boolValue());
    let special = property("com.apple.LSSharedFileList.SpecialItemIdentifier")
        .and_then(|value| value.downcast::<NSString>().ok())
        .and_then(|value| Special::parse(&value.to_string()))
        .or(file_provider.then_some(Special::FileProvider));
    Some(Item {
        visible: number(item, "visibility").is_none_or(|number| number.integerValue() == 0),
        path: item
            .objectForKey(&NSString::from_str("Bookmark"))
            .and_then(|data| data.downcast::<NSData>().ok())
            .and_then(|data| stored_path(&data)),
        name: item
            .objectForKey(&NSString::from_str("Name"))
            .and_then(|name| name.downcast::<NSString>().ok())
            .map(|name| name.to_string()),
        special,
    })
}

fn number(dict: &NSDictionary, key: &str) -> Option<Retained<NSNumber>> {
    dict.objectForKey(&NSString::from_str(key))?
        .downcast::<NSNumber>()
        .ok()
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

    #[test]
    fn this_mac_lists_what_its_finder_shows() {
        let favorites = listed_favorites();
        let locations = listed_locations();
        let titles = |rows: &[Listed]| rows.iter().map(|row| row.title.clone()).collect::<Vec<_>>();
        println!("Favorites: {:?}", titles(&favorites));
        println!("Locations: {:?}", titles(&locations));
        assert!(
            favorites
                .iter()
                .all(|row| row.special != Some(Special::Home)),
            "home is a location"
        );
        assert_eq!(
            locations
                .iter()
                .map(|row| row.special)
                .filter(|s| *s == Some(Special::Network))
                .count(),
            1
        );
    }
}
