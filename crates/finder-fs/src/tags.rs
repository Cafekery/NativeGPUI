//! Finder tags: the user's named colours from Finder's preferences, and the tags on a file.
//!
//! `all` reads Finder's own tag list (name, colour index and order) so the sidebar shows the
//! user's tags, `favorites` the ones its tag menus offer; `of`/`set` read and write a file's tags
//! through the URL resource key Finder uses, so both apps agree.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNumber, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString, NSURL, NSURLTagNamesKey, NSUserDefaults,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// Finder's label colour index: 0 none, 1 grey, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange.
    pub color: u8,
}

/// Finder's seven built-in tags, in the order Finder lists them, unrenamed.
const STANDARD: [(&str, u8); 7] = [
    ("Red", 6),
    ("Orange", 7),
    ("Yellow", 5),
    ("Green", 2),
    ("Blue", 4),
    ("Purple", 3),
    ("Gray", 1),
];

fn standard() -> Vec<Tag> {
    STANDARD
        .iter()
        .map(|&(name, color)| Tag {
            name: name.into(),
            color,
        })
        .collect()
}

/// Every tag the user has, in the order Finder's sidebar lists them.
pub fn all() -> Vec<Tag> {
    match finder_tags().filter(|tags| !tags.is_empty()) {
        Some(tags) => tags,
        None => standard(),
    }
}

/// The tags Finder's tag menus offer, in the order the user dragged them.
pub fn favorites() -> Vec<Tag> {
    let known = all();
    let color = |name: &str| {
        known
            .iter()
            .find(|tag| tag.name == name)
            .map(|tag| tag.color)
            .unwrap_or_default()
    };
    let favorites: Vec<Tag> = favorite_tag_names()
        .unwrap_or_default()
        .into_iter()
        .filter(|name| !name.is_empty())
        .map(|name| Tag {
            color: color(&name),
            name,
        })
        .collect();
    match favorites.is_empty() {
        true => standard(),
        false => favorites,
    }
}

fn favorite_tag_names() -> Option<Vec<String>> {
    let suite = NSString::from_str("com.apple.finder");
    let defaults = NSUserDefaults::alloc();
    let defaults = NSUserDefaults::initWithSuiteName(defaults, Some(&suite))?;
    let key = NSString::from_str("FavoriteTagNames");
    let names = defaults.stringArrayForKey(&key)?;
    Some(names.to_vec().iter().map(|s| s.to_string()).collect())
}

fn plist(bytes: &[u8]) -> Option<Retained<AnyObject>> {
    let data = NSData::with_bytes(bytes);
    unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &data,
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()
}

fn finder_tags() -> Option<Vec<Tag>> {
    let dict = plist(&tag_dict()?)?.downcast::<NSDictionary>().ok()?;
    let dict: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { Retained::cast_unchecked(dict) };
    let entries = dict
        .objectForKey(&NSString::from_str("FinderTags"))?
        .downcast::<NSArray>()
        .ok()?;
    let entries: Retained<NSArray<NSDictionary<NSString, AnyObject>>> =
        unsafe { Retained::cast_unchecked(entries) };
    Some(
        entries
            .to_vec()
            .iter()
            .filter_map(|entry| {
                let name = entry
                    .objectForKey(&NSString::from_str("n"))?
                    .downcast::<NSString>()
                    .ok()?;
                let color = entry
                    .objectForKey(&NSString::from_str("l"))
                    .and_then(|value| value.downcast::<NSNumber>().ok())
                    .map_or(0, |value| value.unsignedCharValue());
                Some(Tag {
                    name: name.to_string(),
                    color,
                })
            })
            .collect(),
    )
}

/// The account's home, not `$HOME`: a fixture home moves an app's folders, never Finder's tags.
fn home() -> Option<PathBuf> {
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    let dir = unsafe { passwd.as_ref() }?.pw_dir;
    if dir.is_null() {
        return None;
    }
    let dir = unsafe { CStr::from_ptr(dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

/// macOS 26 keeps `FinderTagDict` in the iCloud key-value store, not in `SyncedPreferences`.
fn tag_dict() -> Option<Vec<u8>> {
    let stores = home()?.join("Library/Daemon Containers");
    std::fs::read_dir(stores).ok()?.flatten().find_map(|store| {
        let store = store
            .path()
            .join("Data/com.apple.kvs/com.apple.KeyValueService-Production.sqlite");
        let out = Command::new("/usr/bin/sqlite3")
            .arg("-readonly")
            .arg(&store)
            .arg(
                "select hex(ZPLISTDATAVALUE) from ZSYDMANAGEDKEYVALUE where ZKEY = 'FinderTagDict'",
            )
            .output()
            .ok()?;
        let hex = String::from_utf8(out.stdout).ok()?;
        let hex = hex.trim();
        match hex.is_empty() {
            true => None,
            false => hex
                .as_bytes()
                .chunks(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
                .collect(),
        }
    })
}

fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn read_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = CString::new(name).ok()?;
    let size =
        unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let read = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    if read < 0 {
        return None;
    }
    buf.truncate(read as usize);
    Some(buf)
}

/// The colour of each tag name as Finder's `_kMDItemUserTags` xattr records it: `"Name\nN"`.
fn xattr_colors(path: &Path) -> HashMap<String, u8> {
    let mut colors = HashMap::new();
    let Some(bytes) = read_xattr(path, "com.apple.metadata:_kMDItemUserTags") else {
        return colors;
    };
    let Some(Ok(entries)) = plist(&bytes).map(|p| p.downcast::<NSArray>()) else {
        return colors;
    };
    let entries: Retained<NSArray<NSString>> = unsafe { Retained::cast_unchecked(entries) };
    for entry in entries.to_vec() {
        let entry = entry.to_string();
        if let Some((name, color)) = entry.split_once('\n') {
            if let Ok(color) = color.parse() {
                colors.insert(name.to_string(), color);
            }
        }
    }
    colors
}

pub fn of(path: &Path) -> Vec<Tag> {
    let key = unsafe { NSURLTagNamesKey };
    let keys = NSArray::from_slice(&[key]);
    let Ok(values) = url(path).resourceValuesForKeys_error(&keys) else {
        return Vec::new();
    };
    let Some(names) = values
        .objectForKey(key)
        .and_then(|v| v.downcast::<NSArray>().ok())
    else {
        return Vec::new();
    };
    let names: Retained<NSArray<NSString>> = unsafe { Retained::cast_unchecked(names) };
    if names.is_empty() {
        return Vec::new();
    }
    let colors = xattr_colors(path);
    names
        .to_vec()
        .iter()
        .map(|name| {
            let name = name.to_string();
            let color = colors.get(&name).copied().unwrap_or(0);
            Tag { name, color }
        })
        .collect()
}

pub fn set(path: &Path, tags: &[Tag]) -> io::Result<()> {
    let key = unsafe { NSURLTagNamesKey };
    let names: Vec<Retained<NSString>> = tags.iter().map(|t| NSString::from_str(&t.name)).collect();
    let refs: Vec<&NSString> = names.iter().map(|n| &**n).collect();
    let array = NSArray::from_slice(&refs);
    let object: &AnyObject = array.as_ref();
    unsafe { url(path).setResourceValue_forKey_error(Some(object), key) }
        .map_err(|e| io::Error::other(e.localizedDescription().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn set_read_back_and_clear() {
        let dir = std::env::temp_dir().join(format!("finder-fs-tags-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tagged.txt");
        fs::write(&file, "hi").unwrap();

        let mine = vec![
            Tag {
                name: "Reviewed".into(),
                color: 0,
            },
            Tag {
                name: "Urgent".into(),
                color: 0,
            },
        ];
        set(&file, &mine).unwrap();
        let mut read: Vec<String> = of(&file).into_iter().map(|t| t.name).collect();
        read.sort();
        assert_eq!(read, vec!["Reviewed".to_string(), "Urgent".to_string()]);

        set(&file, &[]).unwrap();
        assert!(of(&file).is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_is_never_empty() {
        assert!(!all().is_empty());
        assert!(!favorites().is_empty());
    }
}
