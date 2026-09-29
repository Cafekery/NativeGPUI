use std::ffi::{CStr, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSString, NSURL, NSURLTagNamesKey, NSUserDefaults, ns_string,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub color: u8,
}

const STANDARD: [(&str, u8); 7] = [
    ("Red", 6),
    ("Orange", 7),
    ("Yellow", 5),
    ("Green", 2),
    ("Blue", 4),
    ("Purple", 3),
    ("Gray", 1),
];

fn localized(english: &str, color: u8) -> String {
    crate::words::finder(&format!("TG_COLOR_{color}"), english)
}

fn standard() -> Vec<Tag> {
    STANDARD
        .iter()
        .map(|&(english, color)| Tag {
            name: localized(english, color),
            color,
        })
        .collect()
}

fn standard_color(name: &str) -> Option<u8> {
    STANDARD
        .iter()
        .find(|&&(english, color)| english == name || localized(english, color) == name)
        .map(|&(_, color)| color)
        .or((name == "Grey").then_some(1))
}

fn colored(known: &[Tag], name: String) -> Tag {
    let color = known
        .iter()
        .find(|tag| tag.name == name)
        .map(|tag| tag.color)
        .or_else(|| standard_color(&name))
        .unwrap_or_default();
    Tag { name, color }
}

/// Every tag Finder's sidebar lists, in its order.
pub fn all() -> Vec<Tag> {
    let known = finder_tags().filter(|tags| !tags.is_empty());
    let Some(listed) = crate::sidebar::tags().filter(|tags| !tags.is_empty()) else {
        return known.unwrap_or_else(standard);
    };
    let known = known.unwrap_or_default();
    listed
        .into_iter()
        .map(|(name, _)| colored(&known, name))
        .collect()
}

/// The tags Finder's Tags settings list as favourites, for its menus.
pub fn favorites() -> Vec<Tag> {
    let suite = Some(ns_string!("com.apple.finder"));
    let finder = NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), suite);
    let names: Vec<String> = finder
        .and_then(|finder| finder.stringArrayForKey(ns_string!("FavoriteTagNames")))
        .iter()
        .flat_map(|names| names.iter())
        .map(|name| name.to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if names.is_empty() {
        return standard();
    }
    let known = all();
    names
        .into_iter()
        .map(|name| colored(&known, name))
        .collect()
}

/// The tags Finder's sidebar shows only because they were used lately, not pinned there.
pub fn recent() -> Vec<String> {
    let listed = crate::sidebar::tags().unwrap_or_default();
    listed
        .into_iter()
        .filter_map(|(name, pinned)| (!pinned).then_some(name))
        .collect()
}

fn finder_tags() -> Option<Vec<Tag>> {
    let dict = crate::plist_dict(&tag_dict()?)?;
    let entries = dict
        .objectForKey(ns_string!("FinderTags"))?
        .downcast::<NSArray>()
        .ok()?;
    let tags = entries.iter().filter_map(|entry| {
        let entry = entry.downcast::<NSDictionary>().ok()?;
        let name = entry
            .objectForKey(ns_string!("n"))?
            .downcast::<NSString>()
            .ok()?;
        let color = entry
            .objectForKey(ns_string!("l"))
            .and_then(|color| color.downcast::<NSNumber>().ok())
            .map_or(0, |color| color.unsignedCharValue());
        Some(Tag {
            name: name.to_string(),
            color,
        })
    });
    Some(tags.collect())
}

/// Not `$HOME`: a fixture home moves an app's folders, never Finder's tags.
fn account_home() -> Option<PathBuf> {
    let account = unsafe { libc::getpwuid(libc::getuid()).as_ref() }?;
    if account.pw_dir.is_null() {
        return None;
    }
    let home = unsafe { CStr::from_ptr(account.pw_dir) };
    Some(PathBuf::from(OsStr::from_bytes(home.to_bytes())))
}

/// macOS 26 keeps `FinderTagDict` in the iCloud key-value store, not in `SyncedPreferences`.
fn tag_dict() -> Option<Vec<u8>> {
    let stores = std::fs::read_dir(account_home()?.join("Library/Daemon Containers")).ok()?;
    stores.flatten().find_map(|store| {
        let out = Command::new("/usr/bin/sqlite3")
            .arg("-readonly")
            .arg(
                store
                    .path()
                    .join("Data/com.apple.kvs/com.apple.KeyValueService-Production.sqlite"),
            )
            .arg(
                "select hex(ZPLISTDATAVALUE) from ZSYDMANAGEDKEYVALUE where ZKEY = 'FinderTagDict'",
            )
            .output()
            .ok()?;
        let hex = String::from_utf8(out.stdout).ok()?;
        let hex = hex.trim();
        if hex.is_empty() {
            return None;
        }
        hex.as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect()
    })
}

/// `fileURLWithPath:` alone stats the path to learn whether it is a folder.
fn url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), false)
}

fn user_tags(path: &Path) -> Option<Retained<NSArray>> {
    let path = crate::c_path(path).ok()?;
    let bytes = crate::xattr::get(&path, c"com.apple.metadata:_kMDItemUserTags")?;
    crate::plist(&bytes)?.downcast::<NSArray>().ok()
}

/// The tags the item stores, in its order, from one read of its attribute.
pub fn stored(path: &Path) -> Vec<Tag> {
    let Some(entries) = user_tags(path) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| entry.downcast::<NSString>().ok())
        .map(|entry| {
            let mut name = entry.to_string();
            let mut color = 0;
            if let Some(at) = name.find('\n') {
                color = name[at + 1..].parse().unwrap_or(0);
                name.truncate(at);
            }
            Tag { name, color }
        })
        .collect()
}

pub fn of(path: &Path) -> Vec<Tag> {
    let key = unsafe { NSURLTagNamesKey };
    let Some(names) = url(path)
        .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
        .ok()
        .and_then(|values| values.objectForKey(key)?.downcast::<NSArray>().ok())
        .filter(|names| !names.is_empty())
    else {
        return Vec::new();
    };
    let stored = stored(path);
    names
        .iter()
        .filter_map(|name| name.downcast::<NSString>().ok())
        .map(|name| {
            let name = name.to_string();
            let color = stored
                .iter()
                .rfind(|tag| tag.name == name)
                .map_or(0, |tag| tag.color);
            Tag { name, color }
        })
        .collect()
}

pub fn set(path: &Path, tags: &[Tag]) -> io::Result<()> {
    let names: Vec<_> = tags
        .iter()
        .map(|tag| NSString::from_str(&tag.name))
        .collect();
    let names = NSArray::from_retained_slice(&names);
    unsafe { url(path).setResourceValue_forKey_error(Some(&names), NSURLTagNamesKey) }
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

pub fn toggle(paths: &[PathBuf], tag: &Tag) -> io::Result<()> {
    let held = |tags: &[Tag]| tags.iter().any(|held| held.name == tag.name);
    let carried: Vec<Vec<Tag>> = paths.iter().map(|path| of(path)).collect();
    let everywhere = carried.iter().all(|tags| held(tags));
    for (path, mut tags) in paths.iter().zip(carried) {
        match (everywhere, held(&tags)) {
            (true, _) => tags.retain(|it| it.name != tag.name),
            (false, false) => tags.push(tag.clone()),
            (false, true) => continue,
        }
        set(path, &tags)?;
    }
    Ok(())
}

pub fn apply(paths: &[PathBuf], add: &[Tag], remove: &[String]) -> io::Result<()> {
    for path in paths {
        let before = of(path);
        let mut tags: Vec<Tag> = before
            .iter()
            .filter(|tag| !remove.contains(&tag.name))
            .cloned()
            .collect();
        for tag in add {
            if !tags.iter().any(|held| held.name == tag.name) {
                tags.push(tag.clone());
            }
        }
        if tags != before {
            set(path, &tags)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tag(name: &str) -> Tag {
        Tag {
            name: name.into(),
            color: 0,
        }
    }

    fn names(path: &Path) -> Vec<String> {
        of(path).into_iter().map(|tag| tag.name).collect()
    }

    fn scratch(test: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("finder-fs-{test}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for file in files {
            fs::write(dir.join(file), file).unwrap();
        }
        dir
    }

    #[test]
    fn a_standard_colours_name_says_its_colour_in_either_spelling() {
        assert_eq!(standard_color("Red"), Some(6));
        assert_eq!(standard_color("Gray"), Some(1));
        assert_eq!(standard_color("Grey"), Some(1));
        assert_eq!(standard_color("Work"), None);
    }

    #[test]
    fn set_read_back_and_clear() {
        let dir = scratch("tags", &["tagged.txt"]);
        let file = dir.join("tagged.txt");
        set(&file, &[tag("Reviewed"), tag("Urgent")]).unwrap();
        let mut read = names(&file);
        read.sort();
        assert_eq!(read, ["Reviewed", "Urgent"]);

        set(&file, &[]).unwrap();
        assert!(of(&file).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn all_is_never_empty() {
        assert!(!all().is_empty());
        assert!(!favorites().is_empty());
    }

    #[test]
    fn a_swatch_adds_where_missing_and_clears_where_everywhere() {
        let dir = scratch("toggle", &["a.txt", "b.txt"]);
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        let red = Tag {
            color: 6,
            ..tag("Red")
        };
        set(&a, &[tag("Reviewed")]).unwrap();
        let both = [a.clone(), b.clone()];

        toggle(&both, &red).unwrap();
        assert_eq!(names(&a), ["Reviewed", "Red"]);
        assert_eq!(names(&b), ["Red"]);

        toggle(&both, &red).unwrap();
        assert_eq!(names(&a), ["Reviewed"]);
        assert!(names(&b).is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_edit_touches_only_the_tags_it_names() {
        let dir = scratch("apply", &["a.txt", "b.txt"]);
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        set(&a, &[tag("Reviewed"), tag("Draft"), tag("Red")]).unwrap();
        set(&b, &[tag("Draft")]).unwrap();
        let both = [a.clone(), b.clone()];

        apply(&both, &[tag("Blue")], &["Draft".into()]).unwrap();
        assert_eq!(names(&a), ["Reviewed", "Red", "Blue"]);
        assert_eq!(names(&b), ["Blue"]);
        fs::remove_dir_all(&dir).ok();
    }
}
