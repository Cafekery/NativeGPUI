use std::collections::HashSet;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use block2::StackBlock;
use finder_fs::volumes;
use gpui::App;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidMountNotification, NSWorkspaceDidRenameVolumeNotification,
    NSWorkspaceDidUnmountNotification,
};
use objc2_foundation::{NSFileManager, NSNotification, NSString};

use super::paths::intern;
use crate::app::FinderDemo;
use crate::signal::Signal;

#[derive(PartialEq)]
struct Mounted {
    mount: String,
    name: &'static str,
    available: u64,
    capacity: u64,
    format: &'static str,
    network: bool,
}

/// Free space on screen is at most this stale while it is being drawn.
const FRESH: Duration = Duration::from_millis(4800);

static SEEN: RwLock<Vec<Mounted>> = RwLock::new(Vec::new());
static REFRESHED: Mutex<Option<Instant>> = Mutex::new(None);
/// Rung by a mount, an unmount or a rename, and by free space read from a stale snapshot.
static VOLUMES: Signal = Signal::new();
static MOUNTS: AtomicBool = AtomicBool::new(false);

fn seen() -> RwLockReadGuard<'static, Vec<Mounted>> {
    SEEN.read().unwrap()
}

fn fresh() {
    let mut refreshed = REFRESHED.lock().unwrap();
    if refreshed.is_some_and(|at| at.elapsed() >= FRESH) {
        *refreshed = Some(Instant::now());
        VOLUMES.ring();
    }
}

/// Blocks for milliseconds: never call it from a frame. Whether anything changed.
pub(crate) fn refresh_volumes() -> bool {
    let mounted: Vec<Mounted> = volumes::mounted()
        .iter()
        .map(|volume| Mounted {
            mount: volume.path.to_string_lossy().into_owned(),
            name: intern(&volume.name),
            available: volume.available,
            capacity: volume.capacity,
            format: intern(&volume.format),
            network: volume.network,
        })
        .collect();
    *REFRESHED.lock().unwrap() = Some(Instant::now());
    let mut seen = SEEN.write().unwrap();
    let changed = *seen != mounted;
    *seen = mounted;
    changed
}

pub(crate) fn follow_volumes(cx: &mut App) {
    let block = StackBlock::new(|_: NonNull<NSNotification>| {
        MOUNTS.store(true, Ordering::Relaxed);
        VOLUMES.ring();
    })
    .copy();
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    for name in unsafe {
        [
            NSWorkspaceDidMountNotification,
            NSWorkspaceDidUnmountNotification,
            NSWorkspaceDidRenameVolumeNotification,
        ]
    } {
        unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
    }
    cx.spawn(async move |cx| {
        let mut seen = 0;
        loop {
            seen = VOLUMES.after(seen).await;
            let mounts = MOUNTS.swap(false, Ordering::Relaxed);
            let background = cx.background_executor().clone();
            let changed = background.spawn(async { refresh_volumes() }).await;
            if mounts {
                let windows = cx.update(|cx| {
                    cx.windows()
                        .into_iter()
                        .filter_map(|window| window.downcast::<FinderDemo>())
                        .collect::<Vec<_>>()
                });
                for window in windows {
                    let groups = background
                        .spawn(async { crate::chrome::sidebar::groups() })
                        .await;
                    window
                        .update(cx, |this, _, _| this.sidebar_took(groups))
                        .ok();
                }
            }
            if changed || mounts {
                cx.update(crate::read::notify_windows);
            }
        }
    })
    .detach();
}

fn boot_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = NSFileManager::defaultManager()
            .displayNameAtPath(&NSString::from_str("/"))
            .to_string();
        intern(&name)
    })
}

pub(super) fn volume_of(path: &str) -> (&'static str, &'static str) {
    let under =
        |mount: &str| mount == "/" || path == mount || path.starts_with(&format!("{mount}/"));
    seen()
        .iter()
        .filter(|volume| under(&volume.mount))
        .max_by_key(|volume| volume.mount.len())
        .map(|volume| (intern(&volume.mount), volume.name))
        .unwrap_or(("/", boot_name()))
}

pub(crate) fn volume_roots() -> Vec<&'static str> {
    seen().iter().map(|volume| intern(&volume.mount)).collect()
}

pub(crate) fn network_roots() -> Vec<&'static str> {
    seen()
        .iter()
        .filter(|volume| volume.network)
        .map(|volume| intern(&volume.mount))
        .collect()
}

pub(crate) fn available_bytes(id: &str) -> u64 {
    fresh();
    let (mount, _) = volume_of(id);
    seen()
        .iter()
        .find(|volume| volume.mount == mount)
        .map_or(0, |volume| volume.available)
}

pub(crate) struct VolumeInfo {
    pub(crate) name: &'static str,
    pub(crate) capacity: u64,
    pub(crate) available: u64,
    pub(crate) format: &'static str,
}

pub(crate) fn volume_at(id: &str) -> Option<VolumeInfo> {
    let info = seen()
        .iter()
        .find(|volume| volume.mount == id)
        .map(|volume| VolumeInfo {
            name: volume.name,
            capacity: volume.capacity,
            available: volume.available,
            format: volume.format,
        });
    if info.is_some() {
        fresh();
    }
    info
}

pub(super) fn special() -> &'static HashSet<String> {
    static PLACES: OnceLock<HashSet<String>> = OnceLock::new();
    PLACES.get_or_init(|| {
        use finder_fs::volumes::Standard::*;
        let mut places: HashSet<String> = [
            Home,
            Desktop,
            Documents,
            Downloads,
            Applications,
            Movies,
            Music,
            Pictures,
            Trash,
        ]
        .into_iter()
        .filter_map(volumes::standard)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
        let home = volumes::standard(Home).unwrap_or_default();
        for leaf in ["Public", "Library", "Developer", "Sites", "Applications"] {
            places.insert(home.join(leaf).to_string_lossy().into_owned());
        }
        for path in [
            "/",
            "/Applications/Utilities",
            "/Library",
            "/System",
            "/System/Library",
            "/Users",
            "/Users/Shared",
            "/Volumes",
        ] {
            places.insert(path.to_string());
        }
        places
    })
}
