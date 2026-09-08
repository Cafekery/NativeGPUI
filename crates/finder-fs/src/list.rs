//! Reading one directory: every entry, hidden ones included, in directory order, with an
//! [`Entry`]'s attributes filled in one `getattrlistbulk(2)` pass rather than a `stat` per file.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::entry::{Entry, EntryKind};

/// Not in `libc`: only valid as a `getattrlistbulk` request, one bit above `ATTR_CMN_ADDEDTIME`.
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;
/// `vnode` types (`sys/vnode.h`); `libc` does not expose these.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;
/// `kHasBundle`, a bit in classic `FndrDirInfo.frFlags` (big-endian, offset 8 of `ATTR_CMN_FNDRINFO`).
const HAS_BUNDLE: u16 = 0x2000;

const COMMON_ATTRS: libc::attrgroup_t = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_CRTIME
    | libc::ATTR_CMN_ADDEDTIME
    | libc::ATTR_CMN_FNDRINFO
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;

const OPEN_FLAGS: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY;

struct FdGuard(RawFd);

impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// The directory's own descriptor. `O_DIRECTORY` already rejects a non-directory target, so a
/// symlink to a directory still opens (matching `read_dir`) while a symlink to a file correctly
/// fails with `ENOTDIR`. A path past `PATH_MAX` cannot be opened whole, so it is walked one
/// component at a time with `openat`, which is never handed more than one name.
fn open_dir(dir: &Path) -> io::Result<FdGuard> {
    let path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let fd = unsafe { libc::open(path.as_ptr(), OPEN_FLAGS) };
    if fd >= 0 {
        return Ok(FdGuard(fd));
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENAMETOOLONG) {
        return Err(err);
    }
    let mut at: Option<FdGuard> = None;
    for part in dir.components() {
        let name = CString::new(part.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let parent = at.as_ref().map_or(libc::AT_FDCWD, |open| open.0);
        let fd = unsafe { libc::openat(parent, name.as_ptr(), OPEN_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        at = Some(FdGuard(fd));
    }
    at.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
}

/// Whether `path` is a directory at any depth: `Path::is_dir` stats the whole path, which a path
/// past `PATH_MAX` cannot be.
pub fn is_dir(path: &Path) -> bool {
    open_dir(path).is_ok()
}

/// Every entry of `dir`. Errors are the directory's own (missing, unreadable); a single entry
/// that vanishes mid-read is skipped, not fatal.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    list_dir_batched(dir, usize::MAX, |batch| out.extend(batch))?;
    Ok(out)
}

/// Every directory opened, so a test can hold the index to reading only what it claims to.
#[cfg(test)]
pub(crate) static READS: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// [`list_dir`] delivered in batches of at most `batch` entries, in order.
pub fn list_dir_batched(
    dir: &Path,
    batch: usize,
    mut on_batch: impl FnMut(Vec<Entry>),
) -> io::Result<()> {
    #[cfg(test)]
    READS.lock().unwrap().push(dir.to_path_buf());
    let open = open_dir(dir)?;
    let fd = open.0;

    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON_ATTRS,
        volattr: 0,
        dirattr: libc::ATTR_DIR_ENTRYCOUNT,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    let mut buf = vec![0u8; 256 * 1024];
    let mut pending = Vec::new();
    loop {
        let n = unsafe {
            libc::getattrlistbulk(
                fd,
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
            if let Some(entry) = parse_entry(&buf[off..off + len]) {
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

fn take<'a>(record: &'a [u8], p: &mut usize, n: usize) -> &'a [u8] {
    let s = &record[*p..*p + n];
    *p += n;
    s
}

fn read_timespec(b: &[u8]) -> SystemTime {
    let sec = i64::from_ne_bytes(b[0..8].try_into().unwrap());
    let nsec = i64::from_ne_bytes(b[8..16].try_into().unwrap()) as u32;
    let dur = Duration::new(sec.unsigned_abs(), nsec);
    if sec >= 0 {
        SystemTime::UNIX_EPOCH + dur
    } else {
        SystemTime::UNIX_EPOCH - dur
    }
}

/// One `getattrlistbulk` record: length-prefixed, `attribute_set_t returned` next, then the
/// requested attributes packed in ascending bit order (`FSOPT_PACK_INVAL_ATTRS` omits absent
/// ones entirely, so the offsets are only known by walking `returned` bit by bit).
fn parse_entry(record: &[u8]) -> Option<Entry> {
    let mut p = 4usize;
    let returned_common = u32::from_ne_bytes(record[p..p + 4].try_into().unwrap());
    let returned_dir = u32::from_ne_bytes(record[p + 8..p + 12].try_into().unwrap());
    let returned_file = u32::from_ne_bytes(record[p + 12..p + 16].try_into().unwrap());
    p += 20;

    let mut name = None;
    let mut objtype = VREG;
    let mut created = SystemTime::UNIX_EPOCH;
    let mut modified = SystemTime::UNIX_EPOCH;
    let mut fndrinfo: Option<[u8; 32]> = None;
    let mut flags = 0u32;
    let mut file_id = 0u64;
    let mut added = None;

    // `ATTR_CMN_ERROR`, unlike every other attribute, is packed right after `returned` rather
    // than in bit order, so a failed entry can be recognised before touching the rest.
    if returned_common & ATTR_CMN_ERROR != 0 {
        let code = i32::from_ne_bytes(take(record, &mut p, 4).try_into().unwrap());
        if code != 0 {
            return None;
        }
    }
    if returned_common & libc::ATTR_CMN_NAME != 0 {
        let ref_pos = p;
        let bytes = take(record, &mut p, 8);
        let attr_dataoffset = i32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let attr_length = u32::from_ne_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let start = (ref_pos as i64 + attr_dataoffset as i64) as usize;
        let raw = &record[start..start + attr_length];
        let raw = raw.split(|&b| b == 0).next().unwrap_or(raw);
        name = Some(String::from_utf8_lossy(raw).into_owned());
    }
    if returned_common & libc::ATTR_CMN_OBJTYPE != 0 {
        objtype = u32::from_ne_bytes(take(record, &mut p, 4).try_into().unwrap());
    }
    if returned_common & libc::ATTR_CMN_CRTIME != 0 {
        created = read_timespec(take(record, &mut p, 16));
    }
    if returned_common & libc::ATTR_CMN_MODTIME != 0 {
        modified = read_timespec(take(record, &mut p, 16));
    }
    if returned_common & libc::ATTR_CMN_FNDRINFO != 0 {
        fndrinfo = Some(take(record, &mut p, 32).try_into().unwrap());
    }
    if returned_common & libc::ATTR_CMN_FLAGS != 0 {
        flags = u32::from_ne_bytes(take(record, &mut p, 4).try_into().unwrap());
    }
    if returned_common & libc::ATTR_CMN_FILEID != 0 {
        file_id = u64::from_ne_bytes(take(record, &mut p, 8).try_into().unwrap());
    }
    if returned_common & libc::ATTR_CMN_ADDEDTIME != 0 {
        added = Some(read_timespec(take(record, &mut p, 16)));
    }

    let mut children = None;
    if returned_dir & libc::ATTR_DIR_ENTRYCOUNT != 0 {
        children = Some(u32::from_ne_bytes(
            take(record, &mut p, 4).try_into().unwrap(),
        ));
    }

    let mut size = 0u64;
    if returned_file & libc::ATTR_FILE_DATALENGTH != 0 {
        size = u64::from_ne_bytes(take(record, &mut p, 8).try_into().unwrap());
    }

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
    let hidden = name.starts_with('.') || flags & libc::UF_HIDDEN != 0;
    let package = kind == EntryKind::Directory
        && (fndrinfo.is_some_and(|fi| u16::from_be_bytes([fi[8], fi[9]]) & HAS_BUNDLE != 0)
            || crate::kind::is_package_name(&name));

    Some(Entry {
        name,
        kind,
        size,
        modified,
        created,
        added,
        hidden,
        package,
        file_id,
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, symlink};
    use std::path::PathBuf;

    fn by_name(entries: &[Entry]) -> BTreeMap<&str, &Entry> {
        entries.iter().map(|e| (e.name.as_str(), e)).collect()
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

    /// A directory far past `PATH_MAX`, built one component at a time because `mkdir` cannot be
    /// handed the whole path either.
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
