use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSFileManager, NSNumber, NSString, NSURL};

const CLOUD_DOCS: &str = "com~apple~CloudDocs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFolder {
    pub name: String,
    pub documents: PathBuf,
}

fn containers() -> Option<PathBuf> {
    Some(crate::home()?.join("Library/Mobile Documents"))
}

pub fn drive() -> Option<PathBuf> {
    Some(containers()?.join(CLOUD_DOCS))
}

pub fn is_drive(dir: &Path) -> bool {
    drive().is_some_and(|drive| drive == dir)
}

pub fn app_folders() -> Vec<AppFolder> {
    let Some(read) = containers().and_then(|containers| std::fs::read_dir(containers).ok()) else {
        return Vec::new();
    };
    let mut folders: Vec<AppFolder> = read
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name();
            let id = id.to_str()?;
            if id == CLOUD_DOCS || id.starts_with('.') {
                return None;
            }
            let documents = entry.path().join("Documents");
            if !holds_anything(&documents) {
                return None;
            }
            Some(AppFolder {
                name: published(id)?,
                documents,
            })
        })
        .collect();
    folders.sort_by(|a, b| crate::sort::name_cmp(&a.name, &b.name));
    folders
}

/// The app folder iCloud Drive shows `path` in, if `path` is inside an app's Documents.
pub fn app_folder_of(path: &Path) -> Option<AppFolder> {
    let containers = containers()?;
    let mut parts = path.strip_prefix(&containers).ok()?.components();
    let id = parts.next()?.as_os_str().to_str()?;
    if id == CLOUD_DOCS || parts.next()?.as_os_str() != "Documents" {
        return None;
    }
    Some(AppFolder {
        name: published(id)?,
        documents: containers.join(id).join("Documents"),
    })
}

fn holds_anything(documents: &Path) -> bool {
    std::fs::read_dir(documents).is_ok_and(|mut read| read.next().is_some())
}

fn published(id: &str) -> Option<String> {
    let plist = crate::home()?
        .join("Library/Application Support/CloudDocs/session/containers")
        .join(format!("{}.plist", id.replace('~', ".")));
    let dict = read_plist(&plist)?;
    dict.allValues()
        .into_iter()
        .filter_map(|value| value.downcast::<NSDictionary>().ok())
        .find_map(|entry| {
            // SAFETY: property-list dictionary keys are always strings.
            let entry: Retained<NSDictionary<NSString, AnyObject>> =
                unsafe { Retained::cast_unchecked(entry) };
            let public = entry
                .objectForKey(&NSString::from_str("BRContainerIsDocumentScopePublic"))?
                .downcast::<NSNumber>()
                .ok()?;
            let name = entry
                .objectForKey(&NSString::from_str("BRContainerName"))?
                .downcast::<NSString>()
                .ok()?;
            public.boolValue().then(|| name.to_string())
        })
}

fn read_plist(path: &Path) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    crate::plist_dict(&std::fs::read(path).ok()?)
}

pub fn placeholder_name(name: &str) -> Option<&str> {
    name.strip_prefix('.')?.strip_suffix(".icloud")
}

pub fn placeholder_size(path: &Path) -> Option<u64> {
    let size = read_plist(path)?
        .objectForKey(&NSString::from_str("NSURLFileSizeKey"))?
        .downcast::<NSNumber>()
        .ok()?;
    Some(size.unsignedLongLongValue())
}

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
