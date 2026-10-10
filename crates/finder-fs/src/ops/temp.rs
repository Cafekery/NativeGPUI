use std::ffi::{OsStr, c_int};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2_foundation::NSFileManager;

use super::{NAME_MAX, Restore, deep, dir_of, past_the_lock, trash_one};

const PREFIX: &str = ".finder-fs-";
const STAGING: &str = "staging";
const REPLACE: &str = "replace";
const TRASHES: [&str; 2] = [".Trashes", ".Trash"];
/// Two separators, a slot's name, which stays under 80 bytes, and its item's name.
const SLOT_ROOM: usize = 2 + 80 + NAME_MAX;

/// A process later given the same pid starts at another time.
static STARTED: LazyLock<u64> = LazyLock::new(|| start_of(std::process::id()).unwrap_or(0));

#[derive(Debug, Default)]
pub struct Cleaned {
    pub removed: Vec<PathBuf>,
    /// Items a Replace had swapped out, now in the Trash.
    pub trashed: Vec<Restore>,
    pub failures: Vec<(PathBuf, io::Error)>,
}

/// Temps that transfers no longer running left below `folder`: a copy being written is deleted,
/// an item a Replace had swapped out goes to the Trash.
pub fn clean_temps(folder: &Path) -> Cleaned {
    let mut cleaned = Cleaned::default();
    match std::fs::metadata(folder) {
        Ok(meta) if meta.is_dir() => sweep(folder, Some(meta.dev()), &mut cleaned),
        Ok(_) => cleaned.failures.push((
            folder.to_path_buf(),
            io::Error::from_raw_os_error(libc::ENOTDIR),
        )),
        Err(error) => cleaned.failures.push((folder.to_path_buf(), error)),
    }
    cleaned
}

pub(super) fn clean_in(folder: &Path) {
    sweep(folder, None, &mut Cleaned::default());
}

pub(super) fn staging_path(dest: &Path) -> PathBuf {
    dir_of(dest).join(name(STAGING))
}

/// A folder of its own holds the item a Replace swaps in, and after the swap the item it replaced,
/// so a temp left by a kill says whether it may be deleted.
pub(super) fn replace_slot(dest: &Path) -> io::Result<PathBuf> {
    let holder = room_near(dest).join(name(REPLACE));
    deep::create_dir(&holder, 0o700)?;
    Ok(holder.join(dest.file_name().unwrap_or_default()))
}

/// The Trash takes an item only by a path within `PATH_MAX`, so past it a slot sits higher up.
fn room_near(dest: &Path) -> &Path {
    let dir = dir_of(dest);
    let fits = |at: &&Path| at.as_os_str().len() + SLOT_ROOM < libc::PATH_MAX as usize;
    let Some(near) = dir.ancestors().find(fits).filter(|near| *near != dir) else {
        return dir;
    };
    let volume = |at: &Path| deep::lstat(at).map(|meta| meta.dev()).ok();
    match volume(near).is_some() && volume(near) == volume(dir) {
        true => near,
        false => dir,
    }
}

pub(super) fn trash_slot(fm: &NSFileManager, slot: &Path) -> io::Result<PathBuf> {
    let trashed = trash_one(fm, slot)?;
    let _ = deep::remove_dir(dir_of(slot));
    Ok(trashed)
}

pub(super) fn drop_slot(slot: &Path) {
    let _ = deep::remove(slot);
    let _ = deep::remove_dir(dir_of(slot));
}

fn name(kind: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{PREFIX}{kind}-{}-{}-{n}", std::process::id(), *STARTED)
}

fn start_of(pid: u32) -> Result<u64, i32> {
    let pid = c_int::try_from(pid).map_err(|_| libc::ESRCH)?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if got != size {
        return Err(io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    let info = unsafe { info.assume_init() };
    Ok(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// Another user's process cannot be read, so its temps count as live.
fn running(pid: u32, start: u64) -> bool {
    match start_of(pid) {
        Ok(started) => start == 0 || started == start,
        Err(code) => code != libc::ESRCH,
    }
}

fn parse(name: &OsStr) -> Option<(&str, u32, u64)> {
    let rest = name.to_str()?.strip_prefix(PREFIX)?;
    let (kind, rest) = rest.split_once('-')?;
    let kind = [STAGING, REPLACE]
        .into_iter()
        .find(|known| *known == kind)?;
    let parts: Vec<&str> = rest.split('-').collect();
    let [pid, start, n] = parts[..] else {
        return None;
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !(digits(pid) && digits(start) && digits(n)) {
        return None;
    }
    Some((kind, pid.parse().ok()?, start.parse().ok()?))
}

/// With `dev`, every folder below on that volume too.
fn sweep(folder: &Path, dev: Option<u64>, cleaned: &mut Cleaned) {
    let Ok(entries) = deep::list(folder) else {
        return;
    };
    let mut below = Vec::new();
    for (name, meta) in entries {
        let path = folder.join(&name);
        match parse(&name) {
            Some((kind, pid, start)) if !running(pid, start) => drop_stale(kind, &path, cleaned),
            Some(_) => {}
            None => {
                let inside = dev
                    .is_some_and(|dev| meta.is_ok_and(|meta| meta.is_dir() && meta.dev() == dev));
                if inside && !TRASHES.iter().any(|trash| name == *trash) {
                    below.push(path);
                }
            }
        }
    }
    for path in below {
        sweep(&path, dev, cleaned);
    }
}

fn drop_stale(kind: &str, path: &Path, cleaned: &mut Cleaned) {
    let dropped = match kind {
        STAGING => past_the_lock(path, true, || deep::remove(path))
            .map(|()| cleaned.removed.push(path.to_path_buf())),
        _ => trash_held(path, cleaned),
    };
    if let Err(error) = dropped {
        cleaned.failures.push((path.to_path_buf(), error));
    }
}

fn trash_held(holder: &Path, cleaned: &mut Cleaned) -> io::Result<()> {
    if !deep::lstat(holder)?.is_dir() {
        return Ok(());
    }
    let fm = NSFileManager::defaultManager();
    for (name, _) in deep::list(holder)? {
        let item = trash_one(&fm, &holder.join(&name))?;
        cleaned.trashed.push(Restore {
            item,
            to: dir_of(holder).join(name),
        });
    }
    deep::remove_dir(holder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        child.wait().unwrap();
        child.id()
    }

    #[test]
    fn only_the_exact_names_it_makes_are_read_as_temps() {
        let made = name(STAGING);
        assert_eq!(
            parse(OsStr::new(&made)),
            Some((STAGING, std::process::id(), *STARTED))
        );
        assert!(*STARTED > 0, "this process's start could not be read");
        for other in [
            ".finder-fs-123-4",
            ".finder-fs-staging-12-3",
            ".finder-fs-staging-12-x-3",
            ".finder-fs-staging-+12-3-4",
            ".finder-fs-staging-12-3-4-5",
            ".finder-fs-staging-12-3-4.txt",
            ".finder-fs-moved-12-3-4",
            ".finder-fs-notes.txt",
            "finder-fs-staging-12-3-4",
            ".finder-fs-staging--3-4",
        ] {
            assert_eq!(parse(OsStr::new(other)), None, "{other}");
        }
    }

    #[test]
    fn a_temp_belongs_to_a_process_only_while_that_process_runs() {
        let me = std::process::id();
        assert!(running(me, *STARTED));
        assert!(!running(me, *STARTED + 1), "a reused pid is not the owner");
        assert!(!running(dead_pid(), 1));
    }

    #[test]
    fn a_stale_temp_is_deleted_or_trashed_by_its_kind_and_nothing_else_is_touched() {
        let name_of_disk = format!("XK{}", std::process::id() % 100_000);
        let Some(disk) = crate::ops::transfer::tests::RamDisk::new(32, "HFS+", &name_of_disk)
        else {
            return;
        };
        let dir = disk.mount.join("Folder");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let dead = dead_pid();
        let stale = |kind: &str, n: u32| format!("{PREFIX}{kind}-{dead}-1-{n}");
        let copying = dir.join(stale(STAGING, 0));
        let below = dir.join("sub").join(stale(STAGING, 1));
        let holder = dir.join("sub").join(stale(REPLACE, 2));
        for path in [&copying, &below] {
            fs::write(path, "part of a copy").unwrap();
        }
        fs::create_dir(&holder).unwrap();
        fs::write(holder.join("doc.txt"), "the old doc").unwrap();
        let live = dir.join(name(STAGING));
        let reused = dir.join(format!("{PREFIX}{STAGING}-{}-1-3", std::process::id()));
        fs::write(&live, "being written").unwrap();
        fs::write(&reused, "left by an earlier owner of this pid").unwrap();
        let kept =
            [".finder-fs-123-4", ".finder-fs-notes.txt", "doc.txt"].map(|name| dir.join(name));
        for path in &kept {
            fs::write(path, "the user's").unwrap();
        }

        clean_in(&dir);
        let one_folder = [&copying, &below, &holder].map(|path| path.exists());
        let cleaned = clean_temps(&dir);
        let trashed: Vec<String> = (cleaned.trashed.iter())
            .map(|restore| fs::read_to_string(&restore.item).unwrap_or_default())
            .collect();
        let back_to: Vec<PathBuf> = (cleaned.trashed.iter())
            .map(|restore| restore.to.clone())
            .collect();
        let left: Vec<bool> = [&below, &holder, &reused, &live]
            .map(|path| path.exists())
            .to_vec();
        let untouched = kept
            .iter()
            .all(|path| fs::read_to_string(path).unwrap() == "the user's");

        assert_eq!(
            one_folder,
            [false, true, true],
            "a transfer clears only the folder it lands in"
        );
        assert!(cleaned.failures.is_empty(), "{:?}", cleaned.failures);
        assert_eq!(
            trashed,
            ["the old doc"],
            "a swapped-out item goes to the Trash"
        );
        assert_eq!(back_to, [dir.join("sub").join("doc.txt")]);
        assert_eq!(
            left,
            [false, false, false, true],
            "only a live owner's temp stays"
        );
        assert!(untouched, "a name that is not a temp's was touched");
    }
}
