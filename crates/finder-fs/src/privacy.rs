use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use crate::volumes::is_local;

/// A folder never opened unasked: reading it raises a privacy prompt or leaves this disk.
pub fn off_limits(path: &Path, home: &Path) -> bool {
    elsewhere(path)
        || guarded(path)
        || private(path, home)
        || sandboxed(path, home)
        || simulator(path)
}

fn guarded(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        matches!(
            extension.as_bytes(),
            b"photoslibrary"
                | b"aplibrary"
                | b"migratedphotolibrary"
                | b"musiclibrary"
                | b"tvlibrary"
        )
    })
}

/// An iOS Simulator device, whose `data` sits behind the media-library prompt on the home.
fn simulator(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().as_bytes();
    let uuid_shaped = name.len() == 36
        && name.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    uuid_shaped && path.join("device.plist").is_file()
}

/// Walking `~/Library` raises the media-library prompt and a notice per sandboxed app.
fn sandboxed(path: &Path, home: &Path) -> bool {
    in_a_home(path, home) && path.file_name().unwrap_or_default().as_bytes() == b"Library"
}

fn private(path: &Path, home: &Path) -> bool {
    in_a_home(path, home)
        && matches!(
            path.file_name().unwrap_or_default().as_bytes(),
            b"Desktop" | b"Documents" | b"Downloads" | b"Pictures" | b"Music" | b"Movies"
        )
}

fn in_a_home(path: &Path, home: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    (!home.as_os_str().is_empty() && parent == home) || parent.parent() == Some(Path::new("/Users"))
}

/// Firmlinked volume roots (`/System/Volumes/Data` is `/` again), devices, swap, network mounts.
fn elsewhere(path: &Path) -> bool {
    if matches!(
        path.as_os_str().as_bytes(),
        b"/System/Volumes" | b"/dev" | b"/private/var/vm" | b"/Volumes/.timemachine"
    ) {
        return true;
    }
    path.parent() == Some(Path::new("/Volumes")) && !is_local(path)
}
