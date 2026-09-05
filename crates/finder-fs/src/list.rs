//! Reading one directory.
//!
//! Contract: [`list_dir`] returns every entry of `dir`, hidden ones included, in directory order,
//! with the attributes an [`Entry`] carries filled in one pass. The fast path is
//! `getattrlistbulk(2)` (what Finder and `ls -l` use), so a 20 000-file folder costs
//! milliseconds rather than a `stat` per file; [`list_dir_batched`] hands the same entries over
//! in batches so a view can paint the first rows before the last are read.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use crate::entry::{Entry, EntryKind};

/// Every entry of `dir`. Errors are the directory's own (missing, unreadable); a single entry
/// that vanishes mid-read is skipped, not fatal.
pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    list_dir_batched(dir, usize::MAX, |batch| out.extend(batch))?;
    Ok(out)
}

/// [`list_dir`] delivered in batches of at most `batch` entries, in order.
pub fn list_dir_batched(
    dir: &Path,
    batch: usize,
    mut on_batch: impl FnMut(Vec<Entry>),
) -> io::Result<()> {
    // Placeholder: `read_dir` plus a `stat` per entry. The real implementation is one
    // `getattrlistbulk` loop and must keep this signature and these semantics.
    let mut pending = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let kind = if meta.is_dir() {
            EntryKind::Directory
        } else if meta.file_type().is_symlink() {
            EntryKind::Symlink
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        use std::os::macos::fs::MetadataExt as _;
        use std::os::unix::fs::MetadataExt as _;
        pending.push(Entry {
            hidden: name.starts_with('.') || meta.st_flags() & libc::UF_HIDDEN != 0,
            package: kind == EntryKind::Directory && crate::kind::is_package_name(&name),
            size: if kind == EntryKind::File {
                meta.len()
            } else {
                0
            },
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            created: meta.created().unwrap_or(SystemTime::UNIX_EPOCH),
            added: None,
            file_id: meta.ino(),
            name,
            kind,
        });
        if pending.len() >= batch {
            on_batch(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        on_batch(pending);
    }
    Ok(())
}
