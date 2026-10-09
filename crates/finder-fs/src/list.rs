use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::entry::{Entry, EntryKind};

const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
const HAS_BUNDLE: u16 = 0x2000;
const EXT_HIDDEN: u16 = 0x0010;
const IS_ALIAS: u16 = 0x8000;
pub(crate) const HAS_CUSTOM_ICON: u16 = 0x0400;
const SF_DATALESS: u32 = 0x4000_0000;

const COMMON_ATTRS: libc::attrgroup_t = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_CRTIME
    | libc::ATTR_CMN_ADDEDTIME
    | libc::ATTR_CMN_FNDRINFO
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_USERACCESS
    | libc::ATTR_CMN_FILEID;

const OPEN_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;

struct FdGuard(RawFd);

impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn c_string(name: &OsStr) -> io::Result<CString> {
    Ok(CString::new(name.as_bytes())?)
}

fn open_dir(dir: &Path) -> io::Result<FdGuard> {
    let fd = unsafe { libc::open(c_string(dir.as_os_str())?.as_ptr(), OPEN_FLAGS) };
    if fd >= 0 {
        return Ok(FdGuard(fd));
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENAMETOOLONG) {
        return Err(err);
    }
    let mut at: Option<FdGuard> = None;
    for part in dir.components() {
        let name = c_string(part.as_os_str())?;
        let parent = at.as_ref().map_or(libc::AT_FDCWD, |open| open.0);
        let fd = unsafe { libc::openat(parent, name.as_ptr(), OPEN_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        at = Some(FdGuard(fd));
    }
    at.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

pub fn is_dir(path: &Path) -> bool {
    open_dir(path).is_ok()
}

/// Case as stored and firmlinks folded the way FSEvents reports them, unlike `realpath`.
pub fn real_path(path: &Path) -> Option<PathBuf> {
    let fd = unsafe { libc::open(c_string(path.as_os_str()).ok()?.as_ptr(), libc::O_EVTONLY) };
    if fd < 0 {
        return None;
    }
    let open = FdGuard(fd);
    let mut buf = [0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(open.0, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(OsStr::from_bytes(&buf[..len])))
}

/// Which item a path names, whatever it is called later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    volume: [i32; 2],
    file: u64,
}

pub fn identity(path: &Path) -> Option<Identity> {
    let c_path = c_string(path.as_os_str()).ok()?;
    let mut volume: libc::statfs = unsafe { std::mem::zeroed() };
    let mut item: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut volume) } != 0
        || unsafe { libc::stat(c_path.as_ptr(), &mut item) } != 0
    {
        return None;
    }
    Some(Identity {
        volume: unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(volume.f_fsid) },
        file: item.st_ino,
    })
}

unsafe extern "C" {
    fn fsgetpath(
        buf: *mut libc::c_char,
        size: libc::size_t,
        fsid: *mut libc::fsid_t,
        objid: u64,
    ) -> libc::ssize_t;
}

pub fn path_of(identity: Identity) -> Option<PathBuf> {
    let mut fsid = unsafe { std::mem::transmute::<[i32; 2], libc::fsid_t>(identity.volume) };
    let mut buf = [0u8; libc::PATH_MAX as usize];
    let len = unsafe { fsgetpath(buf.as_mut_ptr().cast(), buf.len(), &mut fsid, identity.file) };
    let len = usize::try_from(len).ok()?;
    let bytes = buf[..len].split(|&b| b == 0).next()?;
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    list_dir_batched(dir, usize::MAX, |batch| out.extend(batch))?;
    Ok(out)
}

/// Falls back to listing the folder past `PATH_MAX` and for an evicted iCloud file's stand-in.
pub fn entry_at(path: &Path) -> io::Result<Entry> {
    entry_with(
        path,
        COMMON_ATTRS & !(ATTR_CMN_ERROR | libc::ATTR_CMN_USERACCESS),
    )
}

/// `entry_at` as a listing of its folder reads it, readability included.
pub fn listed_entry_at(path: &Path) -> io::Result<Entry> {
    entry_with(path, COMMON_ATTRS & !ATTR_CMN_ERROR)
}

fn attr_list(common: libc::attrgroup_t) -> libc::attrlist {
    libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: common,
        volattr: 0,
        dirattr: libc::ATTR_DIR_ENTRYCOUNT,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    }
}

fn entry_with(path: &Path, common: libc::attrgroup_t) -> io::Result<Entry> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let listed = || {
        let wanted = path.file_name().map(|name| name.to_string_lossy());
        list_dir(dir)?
            .into_iter()
            .find(|entry| wanted.as_deref() == Some(entry.name.as_str()))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    };
    let Ok(c_path) = c_string(path.as_os_str()) else {
        return listed();
    };
    let mut list = attr_list(common);
    let mut buf = [0u8; 4096];
    let got = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            &mut list as *mut _ as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            libc::FSOPT_PACK_INVAL_ATTRS | libc::FSOPT_NOFOLLOW,
        )
    };
    match got {
        0 => parse_entry(&buf, dir).map_or_else(listed, Ok),
        _ => listed(),
    }
}

pub fn list_dir_batched(
    dir: &Path,
    batch: usize,
    mut on_batch: impl FnMut(Vec<Entry>),
) -> io::Result<()> {
    let open = open_dir(dir)?;
    let mut list = attr_list(COMMON_ATTRS);
    let mut buf = vec![0u8; 256 * 1024];
    let mut pending = Vec::new();
    loop {
        let n = unsafe {
            libc::getattrlistbulk(
                open.0,
                &mut list as *mut _ as *mut libc::c_void,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                libc::FSOPT_PACK_INVAL_ATTRS as u64,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            break;
        }
        let mut off = 0usize;
        for _ in 0..n {
            let len = u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
            if let Some(entry) = parse_entry(&buf[off..off + len], dir) {
                pending.push(entry);
                if pending.len() >= batch {
                    on_batch(std::mem::take(&mut pending));
                }
            }
            off += len;
        }
    }
    if !pending.is_empty() {
        on_batch(pending);
    }
    Ok(())
}

fn read<const N: usize>(record: &[u8], p: &mut usize) -> [u8; N] {
    let bytes = record[*p..*p + N].try_into().unwrap();
    *p += N;
    bytes
}

fn read_timespec(record: &[u8], p: &mut usize) -> SystemTime {
    let sec = i64::from_ne_bytes(read(record, p));
    let nsec = i64::from_ne_bytes(read(record, p)) as u32;
    let dur = Duration::new(sec.unsigned_abs(), nsec);
    if sec >= 0 {
        SystemTime::UNIX_EPOCH + dur
    } else {
        SystemTime::UNIX_EPOCH - dur
    }
}

/// Attributes are packed in ascending bit order, so each offset is found by walking `returned`.
fn parse_entry(record: &[u8], dir: &Path) -> Option<Entry> {
    let mut p = 4usize;
    let returned_common = u32::from_ne_bytes(read(record, &mut p));
    p += 4;
    let returned_dir = u32::from_ne_bytes(read(record, &mut p));
    let returned_file = u32::from_ne_bytes(read(record, &mut p));
    p += 4;

    let mut name = None;
    let mut objtype = VREG;
    let mut created = SystemTime::UNIX_EPOCH;
    let mut modified = SystemTime::UNIX_EPOCH;
    let mut finder_flags = 0u16;
    let mut flags = 0u32;
    let mut file_id = 0u64;
    let mut readable = true;
    let mut added = None;

    // `ATTR_CMN_ERROR` alone is packed straight after `returned`, not in bit order.
    if returned_common & ATTR_CMN_ERROR != 0 && i32::from_ne_bytes(read(record, &mut p)) != 0 {
        return None;
    }
    if returned_common & libc::ATTR_CMN_NAME != 0 {
        let at = p;
        let offset = i32::from_ne_bytes(read(record, &mut p));
        let length = u32::from_ne_bytes(read(record, &mut p)) as usize;
        let start = (at as i64 + offset as i64) as usize;
        let raw = &record[start..start + length];
        let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
        name = Some(String::from_utf8_lossy(raw).into_owned());
    }
    if returned_common & libc::ATTR_CMN_OBJTYPE != 0 {
        objtype = u32::from_ne_bytes(read(record, &mut p));
    }
    if returned_common & libc::ATTR_CMN_CRTIME != 0 {
        created = read_timespec(record, &mut p);
    }
    if returned_common & libc::ATTR_CMN_MODTIME != 0 {
        modified = read_timespec(record, &mut p);
    }
    if returned_common & libc::ATTR_CMN_FNDRINFO != 0 {
        let info: [u8; 32] = read(record, &mut p);
        finder_flags = u16::from_be_bytes([info[8], info[9]]);
    }
    if returned_common & libc::ATTR_CMN_FLAGS != 0 {
        flags = u32::from_ne_bytes(read(record, &mut p));
    }
    if returned_common & libc::ATTR_CMN_USERACCESS != 0 {
        let access = i32::from_ne_bytes(read(record, &mut p));
        let wanted = match objtype {
            VDIR => libc::R_OK | libc::X_OK,
            _ => libc::R_OK,
        };
        readable = access & wanted == wanted;
    }
    if returned_common & libc::ATTR_CMN_FILEID != 0 {
        file_id = u64::from_ne_bytes(read(record, &mut p));
    }
    if returned_common & libc::ATTR_CMN_ADDEDTIME != 0 {
        added = Some(read_timespec(record, &mut p));
    }
    let children = (returned_dir & libc::ATTR_DIR_ENTRYCOUNT != 0)
        .then(|| u32::from_ne_bytes(read(record, &mut p)));
    let mut size = if returned_file & libc::ATTR_FILE_DATALENGTH != 0 {
        u64::from_ne_bytes(read(record, &mut p))
    } else {
        0
    };

    let name = name?;
    let kind = match objtype {
        VREG => EntryKind::File,
        VDIR => EntryKind::Directory,
        VLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    if kind == EntryKind::Directory {
        size = 0;
    }
    let mut dataless = flags & SF_DATALESS != 0;
    let name = match crate::icloud::placeholder_name(&name) {
        Some(real) => {
            size = crate::icloud::placeholder_size(&dir.join(&name)).unwrap_or(size);
            dataless = true;
            real.to_owned()
        }
        None => name,
    };
    let hidden = name.starts_with('.') || flags & libc::UF_HIDDEN != 0;
    let package = kind == EntryKind::Directory
        && (finder_flags & HAS_BUNDLE != 0 || crate::kind::is_package_name(&name));

    Some(Entry {
        name,
        kind,
        size,
        modified,
        created,
        added,
        hidden,
        package,
        ext_hidden: finder_flags & EXT_HIDDEN != 0,
        alias: kind == EntryKind::File && finder_flags & IS_ALIAS != 0,
        custom_icon: finder_flags & HAS_CUSTOM_ICON != 0,
        dataless,
        readable,
        file_id,
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
    use std::path::PathBuf;

    fn by_name(entries: &[Entry]) -> BTreeMap<&str, &Entry> {
        entries.iter().map(|e| (e.name.as_str(), e)).collect()
    }

    #[test]
    fn a_folder_is_found_by_identity_after_a_rename() {
        let base = std::env::temp_dir().join(format!("finder-fs-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("before")).unwrap();
        let found = identity(&base.join("before")).unwrap();
        fs::rename(base.join("before"), base.join("after")).unwrap();

        let now = path_of(found).unwrap();
        assert_eq!(now, real_path(&base.join("after")).unwrap());
        assert_ne!(now, base.join("after"), "the temp dir is under a symlink");
        fs::remove_dir_all(&base).unwrap();
        assert_eq!(path_of(found), None);
    }

    #[test]
    fn reads_every_kind_of_entry() {
        let dir = std::env::temp_dir().join(format!("finder-fs-list-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plain.txt"), b"hello").unwrap();
        fs::write(dir.join(".dotfile"), b"x").unwrap();
        fs::write(dir.join("hidden-flag"), b"x").unwrap();
        fs::create_dir(dir.join("subdir")).unwrap();
        fs::create_dir(dir.join("stuffed")).unwrap();
        fs::write(dir.join("stuffed/only-child"), b"x").unwrap();
        fs::create_dir(dir.join("Foo.app")).unwrap();
        fs::write(dir.join("héllo-日本語.txt"), b"x").unwrap();
        symlink(dir.join("plain.txt"), dir.join("a-symlink")).unwrap();

        let status = std::process::Command::new("chflags")
            .args(["hidden", dir.join("hidden-flag").to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());

        let entries = list_dir(&dir).unwrap();
        assert_eq!(entries.len(), 8);
        let map = by_name(&entries);

        let plain = map["plain.txt"];
        assert_eq!(plain.kind, EntryKind::File);
        assert_eq!(plain.size, 5);
        assert!(!plain.hidden);
        assert!(!plain.package);
        assert_eq!(
            plain.file_id,
            fs::metadata(dir.join("plain.txt")).unwrap().ino()
        );

        let dotfile = map[".dotfile"];
        assert_eq!(dotfile.kind, EntryKind::File);
        assert!(dotfile.hidden);

        let hidden_flag = map["hidden-flag"];
        assert!(hidden_flag.hidden);

        let subdir = map["subdir"];
        assert_eq!(subdir.kind, EntryKind::Directory);
        assert_eq!(subdir.size, 0);
        assert!(!subdir.package);
        assert_eq!(subdir.children, Some(0));
        assert_eq!(map["stuffed"].children, Some(1));
        assert_eq!(plain.children, None);

        let app = map["Foo.app"];
        assert_eq!(app.kind, EntryKind::Directory);
        assert!(app.package);

        let unicode = map["héllo-日本語.txt"];
        assert_eq!(unicode.kind, EntryKind::File);

        let symlink_entry = map["a-symlink"];
        assert_eq!(symlink_entry.kind, EntryKind::Symlink);

        for e in &entries {
            let meta = fs::symlink_metadata(dir.join(&e.name)).unwrap();
            let modified = meta.modified().unwrap();
            let diff = e
                .modified
                .duration_since(modified)
                .or_else(|err| modified.duration_since(e.modified).map(|_| err.duration()))
                .unwrap();
            assert!(
                diff.as_secs() < 2,
                "{}: {:?} vs {:?}",
                e.name,
                e.modified,
                modified
            );
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_with_a_custom_icon_says_so() {
        let dir = std::env::temp_dir().join(format!("finder-fs-custom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("custom")).unwrap();
        fs::create_dir(dir.join("plain")).unwrap();
        let dog = crate::folder_icon::Decoration::Symbol("dog.fill".into());
        crate::folder_icon::write(&dir.join("custom"), Some(&dog)).unwrap();

        let entries = list_dir(&dir).unwrap();
        let map = by_name(&entries);
        assert!(map["custom"].custom_icon);
        assert!(!map["plain"].custom_icon);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_the_user_cannot_list_is_unreadable() {
        let dir = std::env::temp_dir().join(format!("finder-fs-access-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let modes = [
            ("open", 0o755),
            ("none", 0o000),
            ("search", 0o111),
            ("read", 0o444),
        ];
        for (name, mode) in modes {
            fs::create_dir_all(dir.join(name)).unwrap();
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(mode)).unwrap();
        }
        let entries = list_dir(&dir).unwrap();
        for (name, _) in modes {
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::remove_dir_all(&dir).unwrap();
        let readable: BTreeMap<_, _> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.readable))
            .collect();
        assert_eq!(
            readable,
            BTreeMap::from([
                ("none", false),
                ("open", true),
                ("read", false),
                ("search", false)
            ])
        );
    }

    #[test]
    fn one_entry_reads_like_its_listing() {
        let dir = std::env::temp_dir().join(format!("finder-fs-one-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("stuffed")).unwrap();
        fs::write(dir.join("plain.txt"), b"hello").unwrap();
        fs::write(dir.join("stuffed/child"), b"x").unwrap();
        symlink(dir.join("plain.txt"), dir.join("a-symlink")).unwrap();

        let listed = list_dir(&dir).unwrap();
        assert_eq!(listed.len(), 3);
        for entry in &listed {
            let one = entry_at(&dir.join(&entry.name)).unwrap();
            assert_eq!(
                (&one.name, one.kind, one.size, one.children, one.file_id),
                (
                    &entry.name,
                    entry.kind,
                    entry.size,
                    entry.children,
                    entry.file_id
                ),
            );
            assert_eq!(one.modified, entry.modified, "{}", entry.name);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_evicted_file_is_listed_under_the_name_it_stands_in_for() {
        let dir = std::env::temp_dir().join(format!("finder-fs-icloud-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>NSURLNameKey</key><string>report.pdf</string>\
             <key>NSURLFileSizeKey</key><integer>4096</integer></dict></plist>";
        fs::write(dir.join(".report.pdf.icloud"), plist).unwrap();

        let entries = list_dir(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.name, "report.pdf");
        assert!(entry.dataless);
        assert!(!entry.hidden);
        assert_eq!(entry.size, 4096);

        fs::remove_dir_all(&dir).unwrap();
    }

    fn names_via_read_dir(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn assert_matches_read_dir(dir: &Path) {
        let mut ours: Vec<String> = list_dir(dir).unwrap().into_iter().map(|e| e.name).collect();
        ours.sort();
        assert_eq!(ours, names_via_read_dir(dir), "mismatch listing {:?}", dir);
    }

    #[test]
    fn matches_read_dir_for_usr_bin() {
        assert_matches_read_dir(Path::new("/usr/bin"));
    }

    #[test]
    fn matches_read_dir_for_applications() {
        assert_matches_read_dir(Path::new("/Applications"));
    }

    #[test]
    fn root_lists_without_error() {
        assert!(!list_dir(Path::new("/")).unwrap().is_empty());
    }

    #[test]
    fn missing_dir_is_not_found() {
        let err = list_dir(Path::new("/no/such/directory/at/all")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    fn deep_dir(root: &Path, levels: usize) -> PathBuf {
        fs::create_dir(root).unwrap();
        let mut path = root.to_path_buf();
        let mut at = open_dir(root).unwrap();
        for level in 0..levels {
            let name = CString::new(format!("d{level:04}")).unwrap();
            assert_eq!(unsafe { libc::mkdirat(at.0, name.as_ptr(), 0o755) }, 0);
            let fd = unsafe { libc::openat(at.0, name.as_ptr(), OPEN_FLAGS) };
            assert!(fd >= 0);
            at = FdGuard(fd);
            path.push(name.to_str().unwrap());
        }
        let leaf = CString::new("leaf.txt").unwrap();
        let fd =
            unsafe { libc::openat(at.0, leaf.as_ptr(), libc::O_CREAT | libc::O_WRONLY, 0o644) };
        assert!(fd >= 0);
        unsafe { libc::close(fd) };
        path
    }

    fn remove_deep(root: &Path) {
        assert!(
            std::process::Command::new("rm")
                .args(["-rf".as_ref(), root.as_os_str()])
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn a_directory_past_path_max_lists() {
        let root = std::env::temp_dir().join(format!("finder-fs-deep-{}", std::process::id()));
        remove_deep(&root);
        let deep = deep_dir(&root, 1200);
        assert!(deep.as_os_str().len() > libc::PATH_MAX as usize);
        assert_eq!(
            fs::read_dir(&deep).unwrap_err().raw_os_error(),
            Some(libc::ENAMETOOLONG)
        );

        assert!(is_dir(&deep));
        let entries = list_dir(&deep).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "leaf.txt");
        remove_deep(&root);
    }

    #[test]
    fn batches_respect_the_limit() {
        let dir = std::env::temp_dir().join(format!("finder-fs-batch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..25 {
            fs::write(dir.join(format!("f{i}")), b"x").unwrap();
        }
        let mut batches = Vec::new();
        list_dir_batched(&dir, 10, |b| batches.push(b.len())).unwrap();
        assert_eq!(batches.iter().sum::<usize>(), 25);
        assert!(batches.iter().all(|&n| n <= 10));
        assert!(batches.len() >= 3);
        fs::remove_dir_all(&dir).unwrap();
    }
}
