use std::collections::{BTreeSet, HashSet};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use block2::StackBlock;
use finder_fs::volumes;
use gpui::{App, Window};
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
    mount: &'static str,
    name: &'static str,
    available: u64,
    capacity: u64,
    format: &'static str,
    network: bool,
}

const FRESH: Duration = Duration::from_millis(4800);

static SEEN: RwLock<Vec<Mounted>> = RwLock::new(Vec::new());
static REFRESHED: Mutex<Option<Instant>> = Mutex::new(None);
static VOLUMES: Signal = Signal::new();
static MOUNTS: AtomicBool = AtomicBool::new(false);
static ASKED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
// Kept across launches: sharingd answers nothing for minutes after a mount.
static KNOWN: Mutex<Option<Vec<(String, String, &'static str)>>> = Mutex::new(None);
const KEPT: usize = 64;
// sharingd can take minutes to learn the model of a server mounted in a burst.
const SERVER_WAIT: Duration = Duration::from_secs(300);
static ASKING: AtomicBool = AtomicBool::new(false);

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

// Blocks for milliseconds: never call it from a frame.
pub(crate) fn refresh_volumes() -> bool {
    let volumes = volumes::mounted();
    if ASKING.load(Ordering::Relaxed) {
        ask_servers(&volumes);
    }
    let mounted: Vec<Mounted> = volumes
        .iter()
        .map(|volume| Mounted {
            mount: intern(&volume.path.to_string_lossy()),
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

pub(crate) fn server_symbol(host: &str) -> &'static str {
    let mut known = KNOWN.lock().unwrap();
    let known = known.get_or_insert_with(read_servers);
    let found = known.iter().find(|(known, ..)| known == host);
    found.map_or("display", |&(.., symbol)| symbol)
}

fn ask_servers(volumes: &[volumes::Volume]) {
    let mut asked = ASKED.lock().unwrap();
    asked.retain(|host| {
        volumes
            .iter()
            .any(|volume| volume.server.as_ref() == Some(host))
    });
    let hosts: Vec<String> = volumes
        .iter()
        .filter_map(|volume| volume.server.clone())
        .filter(|host| asked.insert(host.clone()))
        .collect();
    drop(asked);
    if hosts.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        finder_fs::servers::models(&hosts, SERVER_WAIT, |host, model| {
            if remember(host, model) {
                MOUNTS.store(true, Ordering::Relaxed);
                VOLUMES.ring();
            }
        })
    });
}

fn remember(host: &str, model: Option<&str>) -> bool {
    let symbol = model.map(|model| intern(&finder_fs::servers::symbol(Some(model))));
    let mut known = KNOWN.lock().unwrap();
    let known = known.get_or_insert_with(read_servers);
    let was = known
        .iter()
        .position(|(known, ..)| known == host)
        .map(|at| known.remove(at).2);
    if let (Some(model), Some(symbol)) = (model, symbol) {
        known.insert(0, (host.to_owned(), model.to_owned(), symbol));
        known.truncate(KEPT);
    }
    let text: String = known
        .iter()
        .map(|(host, model, symbol)| format!("{host}\t{model}\t{symbol}\n"))
        .collect();
    std::fs::write(crate::store().join("servers.tsv"), text).ok();
    was.unwrap_or("display") != symbol.unwrap_or("display")
}

fn read_servers() -> Vec<(String, String, &'static str)> {
    let text = std::fs::read_to_string(crate::store().join("servers.tsv")).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (host, model) = (fields.next()?.to_owned(), fields.next()?.to_owned());
            Some((host, model, intern(fields.next()?)))
        })
        .take(KEPT)
        .collect()
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
    if let Some(window) = cx.windows().first() {
        // A callback queued before a frame runs before that frame draws.
        let drawn = |window: &mut Window, _: &mut App| {
            window.on_next_frame(|_, _| {
                ASKING.store(true, Ordering::Relaxed);
                VOLUMES.ring();
            })
        };
        window
            .update(cx, |_, window, _| window.on_next_frame(drawn))
            .ok();
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

fn holding<'a>(seen: &'a [Mounted], path: &str) -> Option<&'a Mounted> {
    seen.iter()
        .filter(|volume| {
            volume.mount == "/"
                || (path.strip_prefix(volume.mount))
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .max_by_key(|volume| volume.mount.len())
}

pub(super) fn volume_of(path: &str) -> (&'static str, &'static str) {
    holding(&seen(), path).map_or(("/", boot_name()), |volume| (volume.mount, volume.name))
}

pub(crate) fn volume_roots() -> Vec<&'static str> {
    seen().iter().map(|volume| volume.mount).collect()
}

pub(crate) fn network_roots() -> Vec<&'static str> {
    seen()
        .iter()
        .filter(|volume| volume.network)
        .map(|volume| volume.mount)
        .collect()
}

pub(crate) fn available_bytes(id: &str) -> u64 {
    fresh();
    holding(&seen(), id).map_or(0, |volume| volume.available)
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
