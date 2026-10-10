use std::cmp::Ordering;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use finder_fs::entry::{Entry, EntryKind};
use finder_fs::{comment, flags, kind, list, meta, ops, permissions, sort, tags, volumes};
use objc2_foundation::{NSString, NSURL, NSURLBookmarkResolutionOptions};
use serde_json::{Value, json};

use crate::args::Args;
use crate::out::{self, Out};
use crate::{OK, Stop};

const SF_DATALESS: u32 = 0x4000_0000;
const SF_RESTRICTED: u32 = 0x0008_0000;
const ACL_TYPE_EXTENDED: c_int = 0x0000_0100;

unsafe extern "C" {
    fn acl_get_link_np(path: *const c_char, kind: c_int) -> *mut c_void;
    fn acl_to_text(acl: *mut c_void, len: *mut isize) -> *mut c_char;
    fn acl_free(object: *mut c_void) -> c_int;
}

#[derive(Clone, Copy, PartialEq)]
enum Key {
    Name,
    Kind,
    Size,
    Modified,
    Created,
    Added,
}

impl Key {
    fn parse(text: &str) -> Option<Key> {
        Some(match text {
            "name" => Key::Name,
            "kind" => Key::Kind,
            "size" => Key::Size,
            "modified" | "date-modified" => Key::Modified,
            "created" | "date-created" => Key::Created,
            "added" | "date-added" => Key::Added,
            _ => return None,
        })
    }
}

struct Row {
    entry: Entry,
    kind: kind::Kind,
}

pub fn ls(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let all = args.flag("all");
    let long = args.flag("long");
    let reverse = args.flag("reverse");
    let folders_first = args.flag("folders-first");
    let key = match args.text("sort") {
        None => Key::Name,
        Some(text) => Key::parse(&text).ok_or_else(|| {
            Stop::Usage(format!(
                "--sort takes name, kind, size, modified, created or added, not {text}"
            ))
        })?,
    };
    args.done("ls")?;
    let mut dirs = args.paths();
    if dirs.is_empty() {
        dirs.push(crate::args::clean(".".as_ref()));
    }
    let many = dirs.len() > 1;
    for (at, dir) in dirs.iter().enumerate() {
        let entries = match list::is_dir(dir) {
            true => list::list_dir(dir),
            false => list::entry_at(dir).map(|entry| vec![entry]),
        }
        .map_err(|error| Stop::Refused(format!("{}: {error}", dir.display())))?;
        let base = match list::is_dir(dir) {
            true => dir.as_path(),
            false => dir.parent().unwrap_or(Path::new("/")),
        };
        let rows: Vec<Row> = (entries.into_iter())
            .filter(|entry| all || !entry.hidden)
            .map(|entry| Row {
                kind: kind::kind_of(&entry),
                entry,
            })
            .collect();
        let order = ordered(&rows, key, reverse, folders_first);
        if out.json {
            let listed: Vec<Value> = order.iter().map(|&at| row_json(&rows[at], base)).collect();
            out.emit(json!({ "event": "listing", "path": out::path(dir), "entries": listed }));
            continue;
        }
        if many {
            out.say(&format!(
                "{}{}:",
                if at > 0 { "\n" } else { "" },
                dir.display()
            ));
        }
        for &at in &order {
            let row = &rows[at];
            let name = finder_fs::name::display(&row.entry.name).into_owned();
            match long {
                false => out.say(&name),
                true => out.say(&format!(
                    "{}  {:>9}  {:<24}  {name}",
                    local_time(row.entry.modified),
                    match row.entry.kind == EntryKind::Directory && !row.entry.package {
                        true => "--".into(),
                        false => out::size(row.entry.size),
                    },
                    row.kind.label,
                )),
            }
        }
    }
    Ok(OK)
}

fn ordered(rows: &[Row], key: Key, reverse: bool, folders_first: bool) -> Vec<usize> {
    let size = |row: &Row| (!row.entry.is_dir()).then_some(row.entry.size);
    let by = |a: usize, b: usize| {
        let (one, other) = (&rows[a], &rows[b]);
        let order = match key {
            Key::Name => Ordering::Equal,
            Key::Kind => sort::name_cmp(&one.kind.label, &other.kind.label),
            Key::Size => size(one).cmp(&size(other)),
            Key::Modified => one.entry.modified.cmp(&other.entry.modified),
            Key::Created => one.entry.created.cmp(&other.entry.created),
            Key::Added => one.entry.added.cmp(&other.entry.added),
        };
        let order = if reverse { order.reverse() } else { order };
        match folders_first {
            true => other.entry.is_dir().cmp(&one.entry.is_dir()).then(order),
            false => order,
        }
    };
    sort::order(rows.len(), by, |at| rows[at].entry.name.as_str(), reverse)
}

fn row_json(row: &Row, dir: &Path) -> Value {
    let entry = &row.entry;
    json!({
        "name": entry.name,
        "path": out::path(&entry.path_in(dir)),
        "display": finder_fs::name::display(&entry.name),
        "type": type_name(entry.kind),
        "kind": &*row.kind.label,
        "uti": &*row.kind.uti,
        "package": entry.package,
        "size": entry.size,
        "children": entry.children,
        "modified": seconds(entry.modified),
        "created": seconds(entry.created),
        "added": entry.added.map(seconds),
        "hidden": entry.hidden,
        "extension_hidden": entry.ext_hidden,
        "alias": entry.alias,
        "custom_icon": entry.custom_icon,
        "dataless": entry.dataless,
        "readable": entry.readable,
    })
}

fn type_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Directory => "folder",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "other",
    }
}

pub fn info(mut args: Args, out: &mut Out) -> Result<u8, Stop> {
    let shallow = args.flag("shallow");
    args.done("info")?;
    let paths = args.paths();
    if paths.is_empty() {
        return Err(Stop::Usage("info needs a path".into()));
    }
    let mut code = OK;
    for path in &paths {
        let facts = match facts(path, !shallow) {
            Ok(facts) => facts,
            Err(error) => {
                out.warn(&format!("{}: {error}", path.display()));
                code = crate::PARTIAL;
                continue;
            }
        };
        match out.json {
            true => out.emit(facts),
            false => print_facts(&facts, out),
        }
    }
    if code != OK && paths.len() == 1 {
        return Ok(crate::REFUSED);
    }
    Ok(code)
}

fn facts(path: &Path, deep: bool) -> std::io::Result<Value> {
    let meta = std::fs::symlink_metadata(path)?;
    let entry = list::entry_at(path)?;
    let kind = kind::kind_of_path(path);
    let (bytes, allocated, items) = match meta.is_dir() && deep {
        true => tally(path),
        false => (meta.len(), meta.blocks() * 512, 0),
    };
    let grants = permissions::grants(path).unwrap_or_default();
    let found = meta::meta(path);
    Ok(json!({
        "event": "info",
        "path": out::path(path),
        "name": entry.name,
        "display": finder_fs::name::of_path(path),
        "type": type_name(entry.kind),
        "kind": &*kind.label,
        "uti": &*kind.uti,
        "package": entry.package,
        "size": bytes,
        "on_disk": allocated,
        "items": (meta.is_dir() && deep).then_some(items),
        "created": seconds(entry.created),
        "modified": seconds(entry.modified),
        "added": entry.added.map(seconds),
        "last_opened": found.last_used,
        "tags": tags::of(path).iter().map(|tag| json!({ "name": tag.name, "color": tag.color })).collect::<Vec<_>>(),
        "comment": comment::of(path),
        "locked": ops::locked(path),
        "hidden": entry.hidden,
        "extension_hidden": entry.ext_hidden,
        "stationery": flags::of(path) & flags::IS_STATIONERY != 0,
        "custom_icon": entry.custom_icon,
        "alias": entry.alias,
        "alias_of": entry.alias.then(|| alias_target(path)).flatten().map(|to| out::path(&to)),
        "link_to": meta.is_symlink().then(|| std::fs::read_link(path).ok()).flatten().map(|to| out::path(&to)),
        "mode": format!("{:o}", meta.mode() & 0o7777),
        "owner": grants.first().map(|grant| grant.name.clone()),
        "group": (grants.iter()).find(|grant| grant.class == Some(permissions::Class::Group)).map(|grant| grant.name.clone()),
        "you": format!("{:?}", permissions::yours(path)),
        "grants": grants.iter().map(|grant| json!({
            "name": grant.name,
            "group": grant.group,
            "privilege": format!("{:?}", grant.privilege),
            "class": grant.class.map(|class| format!("{class:?}")),
        })).collect::<Vec<_>>(),
        "acl": acl_text(path),
        "flags": flag_names(meta.st_flags()),
        "xattrs": xattrs(path).into_iter().map(|(name, size)| json!({ "name": name, "size": size })).collect::<Vec<_>>(),
        "links": meta.nlink(),
        "inode": meta.ino(),
        "device": meta.dev(),
        "volume": volume_of(path).map(|volume| json!({
            "name": volume.name,
            "path": out::path(&volume.path),
            "format": volume.format,
        })),
        "pixels": found.pixels.map(|(w, h)| json!([w, h])),
        "pages": found.pages,
        "duration": found.duration,
    }))
}

fn print_facts(facts: &Value, out: &mut Out) {
    const SHOWN: [&str; 21] = [
        "path",
        "display",
        "kind",
        "size",
        "on_disk",
        "items",
        "created",
        "modified",
        "added",
        "last_opened",
        "tags",
        "comment",
        "locked",
        "hidden",
        "custom_icon",
        "alias_of",
        "link_to",
        "mode",
        "owner",
        "acl",
        "xattrs",
    ];
    for key in SHOWN {
        let value = &facts[key];
        let text = match (key, value) {
            (_, Value::Null) => continue,
            ("size" | "on_disk", Value::Number(bytes)) => {
                let bytes = bytes.as_u64().unwrap_or(0);
                format!("{} ({bytes} bytes)", out::size(bytes))
            }
            ("created" | "modified" | "added" | "last_opened", Value::Number(at)) => {
                let at = at.as_f64().unwrap_or(0.);
                local_time(UNIX_EPOCH + std::time::Duration::from_secs_f64(at.max(0.)))
            }
            ("tags", Value::Array(tags)) if tags.is_empty() => continue,
            ("tags", Value::Array(tags)) => (tags.iter())
                .filter_map(|tag| tag["name"].as_str())
                .collect::<Vec<_>>()
                .join(", "),
            ("xattrs", Value::Array(names)) if names.is_empty() => continue,
            ("xattrs", Value::Array(names)) => (names.iter())
                .map(|xattr| {
                    format!(
                        "{} ({})",
                        xattr["name"].as_str().unwrap_or(""),
                        xattr["size"]
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            ("comment" | "acl", Value::String(text)) if text.is_empty() => continue,
            ("acl", Value::String(text)) => text.trim().replace('\n', "; "),
            (_, Value::String(text)) => text.clone(),
            (_, value) => value.to_string(),
        };
        out.say(&format!("{key:>12}: {text}"));
    }
    if let Some(owner) = facts["owner"].as_str() {
        let group = facts["group"].as_str().unwrap_or("");
        out.say(&format!("{:>12}: {owner}:{group}", "owned by"));
    }
}

/// Logical bytes, allocated bytes and entries below a folder, not following links.
fn tally(dir: &Path) -> (u64, u64, u64) {
    let mut sums = (0, 0, 0);
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            sums.2 += 1;
            if meta.is_dir() {
                pending.push(entry.path());
                continue;
            }
            sums.0 += meta.len();
            sums.1 += meta.blocks() * 512;
        }
    }
    sums
}

fn alias_target(path: &Path) -> Option<PathBuf> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let resolved = NSURL::URLByResolvingAliasFileAtURL_options_error(
        &url,
        NSURLBookmarkResolutionOptions::WithoutUI | NSURLBookmarkResolutionOptions::WithoutMounting,
    )
    .ok()?;
    resolved.path().map(|path| PathBuf::from(path.to_string()))
}

fn acl_text(path: &Path) -> String {
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return String::new();
    };
    let acl = unsafe { acl_get_link_np(name.as_ptr(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        return String::new();
    }
    let text = unsafe { acl_to_text(acl, std::ptr::null_mut()) };
    let shown = match text.is_null() {
        true => String::new(),
        false => unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned(),
    };
    unsafe {
        if !text.is_null() {
            acl_free(text.cast());
        }
        acl_free(acl);
    }
    shown
}

fn flag_names(bits: u32) -> Vec<&'static str> {
    const NAMES: [(u32, &str); 9] = [
        (libc::UF_NODUMP, "nodump"),
        (libc::UF_IMMUTABLE, "uchg"),
        (libc::UF_APPEND, "uappnd"),
        (libc::UF_HIDDEN, "hidden"),
        (libc::UF_COMPRESSED, "compressed"),
        (libc::SF_IMMUTABLE, "schg"),
        (libc::SF_APPEND, "sappnd"),
        (SF_RESTRICTED, "restricted"),
        (SF_DATALESS, "dataless"),
    ];
    NAMES
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

pub fn xattrs(path: &Path) -> Vec<(String, usize)> {
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return Vec::new();
    };
    let list = |buffer: *mut c_char, size: usize| unsafe {
        libc::listxattr(name.as_ptr(), buffer, size, libc::XATTR_NOFOLLOW)
    };
    let Ok(len) = usize::try_from(list(std::ptr::null_mut(), 0)) else {
        return Vec::new();
    };
    let mut names = vec![0u8; len];
    let Ok(len) = usize::try_from(list(names.as_mut_ptr().cast(), len)) else {
        return Vec::new();
    };
    names[..len]
        .split(|&byte| byte == 0)
        .filter(|attr| !attr.is_empty())
        .map(|attr| {
            let attr_name = std::ffi::CString::new(attr).unwrap_or_default();
            let size = unsafe {
                libc::getxattr(
                    name.as_ptr(),
                    attr_name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            };
            (
                String::from_utf8_lossy(attr).into_owned(),
                usize::try_from(size).unwrap_or(0),
            )
        })
        .collect()
}

fn volume_of(path: &Path) -> Option<volumes::Volume> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    volumes::mounted()
        .into_iter()
        .filter(|volume| real.starts_with(&volume.path))
        .max_by_key(|volume| volume.path.as_os_str().len())
}

fn seconds(at: SystemTime) -> f64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

fn local_time(at: SystemTime) -> String {
    let secs = seconds(at) as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut parts) }.is_null() {
        return "?".into();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        parts.tm_year + 1900,
        parts.tm_mon + 1,
        parts.tm_mday,
        parts.tm_hour,
        parts.tm_min
    )
}
