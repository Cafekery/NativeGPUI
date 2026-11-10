use std::io;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSFileManager, NSNumber, NSString, NSURL, ns_string};

const CLOUD_DOCS: &str = "com~apple~CloudDocs";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppFolder {
    pub name: String,
    pub documents: PathBuf,
}

impl AppFolder {
    pub fn app(&self) -> Option<String> {
        let id = self.documents.parent()?.file_name()?.to_str()?;
        Some(app_of(id))
    }
}

fn app_of(container: &str) -> String {
    let id = container
        .split_once('~')
        .filter(|(team, _)| {
            team.len() == 10
                && team
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
        .map_or(container, |(_, id)| id);
    id.strip_prefix("iCloud~").unwrap_or(id).replace('~', ".")
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
            let public = entry
                .objectForKey(ns_string!("BRContainerIsDocumentScopePublic"))?
                .downcast::<NSNumber>()
                .ok()?;
            let name = entry
                .objectForKey(ns_string!("BRContainerName"))?
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
        .objectForKey(ns_string!("NSURLFileSizeKey"))?
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
    fn a_container_names_the_app_that_owns_it() {
        assert_eq!(app_of("com~apple~Pages"), "com.apple.Pages");
        assert_eq!(app_of("iCloud~md~obsidian"), "md.obsidian");
        assert_eq!(
            app_of("4R6749AYRE~com~pixelmatorteam~pixelmator"),
            "com.pixelmatorteam.pixelmator"
        );
        assert_eq!(
            app_of("777W53UFB2~iCloud~com~burbn~instagram"),
            "com.burbn.instagram"
        );
    }

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
