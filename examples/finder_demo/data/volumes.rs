use std::collections::{BTreeSet, HashSet};
use std::net::IpAddr;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use block2::StackBlock;
use finder_fs::servers::same_host;
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

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Mounted {
    mount: &'static str,
    pub(crate) name: &'static str,
    pub(crate) available: u64,
    pub(crate) capacity: u64,
    pub(crate) format: &'static str,
    pub(crate) network: bool,
    pub(crate) ejectable: bool,
    pub(crate) server: Option<&'static str>,
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
            ejectable: volume.ejectable,
            server: volume.server.as_deref().map(intern),
        })
        .collect();
    *REFRESHED.lock().unwrap() = Some(Instant::now());
    let mut seen = SEEN.write().unwrap();
    let changed = *seen != mounted;
    *seen = mounted;
    changed
}

fn known_server(host: &str) -> Option<(String, &'static str)> {
    let mut known = KNOWN.lock().unwrap();
    let known = known.get_or_insert_with(read_servers);
    let found = known.iter().find(|(known, ..)| same_host(known, host))?;
    Some((found.1.clone(), found.2))
}

pub(crate) fn server_symbol(host: &str) -> &'static str {
    known_server(host).map_or("display", |(_, symbol)| symbol)
}

pub(crate) fn server_model(host: &str) -> Option<String> {
    known_server(host).map(|(model, _)| model)
}

fn ask_servers(volumes: &[volumes::Volume]) {
    // Neither Bonjour nor sharingd ever answers for a bare address.
    let reached: Vec<String> = volumes
        .iter()
        .filter_map(|volume| volume.server.clone())
        .chain(
            crate::remote::servers()
                .iter()
                .map(|server| server.host.to_owned()),
        )
        .filter(|host| {
            host.split('%')
                .next()
                .unwrap_or(host)
                .parse::<IpAddr>()
                .is_err()
        })
        .collect();
    let mut asked = ASKED.lock().unwrap();
    asked.retain(|host| reached.iter().any(|server| same_host(server, host)));
    let mut hosts: Vec<String> = Vec::new();
    for host in reached {
        if !asked
            .iter()
            .chain(&hosts)
            .any(|known| same_host(known, &host))
        {
            hosts.push(host);
        }
    }
    asked.extend(hosts.iter().cloned());
    drop(asked);
    if hosts.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let mut answered = Vec::new();
        finder_fs::servers::models(&hosts, SERVER_WAIT, |host, model| {
            answered.push(host.to_owned());
            if remember(host, model) {
                mounts_changed();
            }
        });
        // A host that stayed silent is asked again on the next refresh.
        let mut asked = ASKED.lock().unwrap();
        for host in hosts {
            if !answered.contains(&host) {
                asked.remove(&host);
            }
        }
    });
}

// A host that answered without a model keeps the one it gave last time.
fn remember(host: &str, model: Option<&str>) -> bool {
    let Some(model) = model else {
        return false;
    };
    let model = model.replace(['\t', '\n'], " ");
    let symbol = intern(&finder_fs::servers::symbol(Some(&model)));
    let mut known = KNOWN.lock().unwrap();
    let known = known.get_or_insert_with(read_servers);
    let was = known
        .iter()
        .position(|(known, ..)| same_host(known, host))
        .map(|at| known.remove(at));
    let changed = was.is_none_or(|(_, old, drawn)| old != model || drawn != symbol);
    known.insert(0, (host.to_owned(), model, symbol));
    known.truncate(KEPT);
    let text: String = known
        .iter()
        .map(|(host, model, symbol)| format!("{host}\t{model}\t{symbol}\n"))
        .collect();
    std::fs::write(crate::store().join("servers.tsv"), text).ok();
    changed
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

// Asks once the first window has drawn, so the lookups never delay first paint.
pub(crate) fn ask_servers_once_drawn(window: &mut Window) {
    if ASKING.load(Ordering::Relaxed) {
        return;
    }
    // A callback queued before a frame runs before that frame draws.
    window.on_next_frame(|window, _| {
        window.on_next_frame(|_, _| {
            ASKING.store(true, Ordering::Relaxed);
            VOLUMES.ring();
        })
    });
}

pub(crate) fn mounts_changed() {
    MOUNTS.store(true, Ordering::Relaxed);
    VOLUMES.ring();
}

pub(crate) fn follow_volumes(cx: &mut App) {
    let block = StackBlock::new(|_: NonNull<NSNotification>| mounts_changed()).copy();
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
                        .update(cx, |this, _, cx| {
                            this.sidebar_took(groups);
                            if this.folders.has(crate::locations::COMPUTER) {
                                this.read_location(crate::locations::COMPUTER, cx);
                            }
                        })
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

pub(crate) fn is_volume_root(id: &str) -> bool {
    seen().iter().any(|volume| volume.mount == id)
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

pub(crate) fn volume_at(id: &str) -> Option<Mounted> {
    let volume = seen().iter().find(|volume| volume.mount == id).copied();
    if volume.is_some() {
        fresh();
    }
    volume
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
