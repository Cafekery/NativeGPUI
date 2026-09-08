//! iCloud Drive as Finder shows it: the folder CloudDocs keeps on disk, the app folders that
//! live beside it rather than inside it, and the items iCloud has evicted.

use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSData, NSDictionary, NSFileManager, NSNumber, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString, NSURL,
};

/// The container every other one is shown beside.
const CLOUD_DOCS: &str = "com~apple~CloudDocs";

/// What an evicted file leaves on disk in place of its data.
const PLACEHOLDER: &str = ".icloud";

/// A folder Finder lists inside iCloud Drive that is not inside it on disk: one app's own
/// container, under the name the app published it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFolder {
    pub name: String,
    pub documents: PathBuf,
}

fn containers() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Mobile Documents"))
}

/// `~/Library/Mobile Documents/com~apple~CloudDocs`, whether or not the account has one.
pub fn drive() -> Option<PathBuf> {
    Some(containers()?.join(CLOUD_DOCS))
}

pub fn is_drive(dir: &Path) -> bool {
    drive().is_some_and(|drive| drive == dir)
}

/// The app folders Finder shows in iCloud Drive: a container that published its Documents folder
/// and has something in it, named as its app named it.
pub fn app_folders() -> Vec<AppFolder> {
    let Some(containers) = containers() else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(&containers) else {
        return Vec::new();
    };
    let mut folders: Vec<AppFolder> = read
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id == CLOUD_DOCS || id.starts_with('.') {
                return None;
            }
            let documents = entry.path().join("Documents");
            published(&id)
                .filter(|_| holds_anything(&documents))
                .map(|name| AppFolder { name, documents })
        })
        .collect();
    folders.sort_by(|a, b| crate::sort::name_cmp(&a.name, &b.name));
    folders
}

/// Finder shows a published container once it holds anything at all, a hidden file included,
/// and never while its Documents folder is empty.
fn holds_anything(documents: &Path) -> bool {
    std::fs::read_dir(documents).is_ok_and(|mut read| read.next().is_some())
}

/// The name an app published its container under, if it published it at all. CloudDocs records
/// both beside the container it manages, keyed by the bundle the container belongs to.
fn published(id: &str) -> Option<String> {
    let plist = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Application Support/CloudDocs/session/containers")
        .join(format!("{}.plist", id.replace('~', ".")));
    let dict = read_plist(&plist)?;
    dict.allValues()
        .into_iter()
        .filter_map(|value| value.downcast::<NSDictionary>().ok())
        .find_map(|entry| {
            let entry: Retained<NSDictionary<NSString, AnyObject>> =
                unsafe { Retained::cast_unchecked(entry) };
            let public = entry
                .objectForKey(&NSString::from_str("BRContainerIsDocumentScopePublic"))
                .and_then(|value| value.downcast::<NSNumber>().ok())?;
            let name = entry
                .objectForKey(&NSString::from_str("BRContainerName"))
                .and_then(|value| value.downcast::<NSString>().ok())?;
            public.boolValue().then(|| name.to_string())
        })
}

fn read_plist(path: &Path) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    let bytes = std::fs::read(path).ok()?;
    let data = NSData::with_bytes(&bytes);
    let plist = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    let dict = plist.downcast::<NSDictionary>().ok()?;
    Some(unsafe { Retained::cast_unchecked(dict) })
}

/// The name an evicted item is shown under: `.report.pdf.icloud` on disk is `report.pdf`.
pub fn placeholder_name(name: &str) -> Option<&str> {
    name.strip_prefix('.')?.strip_suffix(PLACEHOLDER)
}

/// The size the placeholder records for the data it stands in for.
pub fn placeholder_size(path: &Path) -> Option<u64> {
    let dict = read_plist(path)?;
    let size = dict
        .objectForKey(&NSString::from_str("NSURLFileSizeKey"))
        .and_then(|value| value.downcast::<NSNumber>().ok())?;
    Some(size.unsignedLongLongValue())
}

/// Asks iCloud for an evicted item's data, as opening it in Finder does.
pub fn download(path: &Path) -> io::Result<()> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .startDownloadingUbiquitousItemAtURL_error(&url)
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_names_the_file_it_stands_in_for() {
        assert_eq!(placeholder_name(".report.pdf.icloud"), Some("report.pdf"));
        assert_eq!(placeholder_name("report.pdf"), None);
        assert_eq!(placeholder_name(".report.pdf"), None);
    }

    #[test]
    fn every_app_folder_has_a_documents_directory_with_something_in_it() {
        for folder in app_folders() {
            assert!(folder.documents.is_dir(), "{:?}", folder.documents);
            assert!(holds_anything(&folder.documents), "{:?}", folder.documents);
            assert!(!folder.name.is_empty());
        }
    }
}
