use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_void};
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, LazyLock};

use finder_fs::servers::Computer;
use gpui::{App, Context, SharedString, Window};
use native_gpui::tokens::FileType;
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use crate::app::FinderDemo;
use crate::data::{self, Face, Line, Row, intern};
use crate::signal::Signal;

pub(crate) const COMPUTER: &str = "computer:";
pub(crate) const NETWORK: &str = "network:";
pub(crate) const AIRDROP: &str = "airdrop:";
pub(crate) const SHARED: &str = "shared:";

const AIRDROP_APP: &str =
    "/System/Library/CoreServices/Finder.app/Contents/Applications/AirDrop.app";

const HITS: usize = 200;

pub(crate) fn is_location(id: &str) -> bool {
    matches!(id, COMPUTER | NETWORK | AIRDROP | SHARED)
}

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *mut NSString;
}

extern "C" {
    fn sysctlbyname(
        name: *const c_char,
        value: *mut c_void,
        size: *mut usize,
        new: *const c_void,
        new_size: usize,
    ) -> i32;
}

fn sysctl(name: &CStr) -> Option<Vec<u8>> {
    let mut size = 0;
    let asked = unsafe { sysctlbyname(name.as_ptr(), ptr::null_mut(), &mut size, ptr::null(), 0) };
    let mut value = vec![0; size];
    let read = unsafe {
        sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut size,
            ptr::null(),
            0,
        )
    };
    (asked == 0 && read == 0).then(|| {
        value.truncate(size);
        value
    })
}

fn sysctl_text(name: &CStr) -> String {
    let value = sysctl(name).unwrap_or_default();
    String::from_utf8_lossy(&value)
        .trim_end_matches('\0')
        .to_owned()
}

fn this_mac() -> FileType<'static> {
    static TYPE: LazyLock<&'static str> = LazyLock::new(|| {
        let model = sysctl_text(c"hw.model");
        finder_fs::servers::model_type(Some(&model))
            .map_or("com.apple.mac", |(uti, _)| intern(&uti))
    });
    FileType::Uti(*TYPE)
}

pub(crate) fn artwork(id: &str) -> Option<FileType<'static>> {
    match id {
        COMPUTER => Some(this_mac()),
        NETWORK => Some(FileType::Named("NSNetwork")),
        AIRDROP => Some(FileType::Path(AIRDROP_APP)),
        _ => None,
    }
}

/// The line an Info window describes for a location Finder gives one.
pub(crate) fn info_line(id: &'static str) -> Option<Line> {
    let (title, _) = place(id)?;
    let kind = match id {
        NETWORK => "Neighborhood",
        AIRDROP => "AirDrop",
        _ => "",
    };
    let row = location_row(
        id,
        title.into(),
        finder_fs::words::say(kind).into_owned().into(),
        "",
        Some(id),
        artwork(id)?,
    );
    Some(Line {
        key: row.path.clone(),
        row,
        depth: 0,
    })
}

/// Finder's Info window gives the computer its processor and memory, the others their kind.
pub(crate) fn info_fields(row: &Row) -> Option<Vec<(&'static str, SharedString)>> {
    static COMPUTER_FIELDS: LazyLock<[(&str, SharedString); 2]> = LazyLock::new(|| {
        let memory = sysctl(c"hw.memsize")
            .and_then(|bytes| Some(u64::from_ne_bytes(bytes.try_into().ok()?)))
            .unwrap_or(0);
        [
            ("Processor", sysctl_text(c"machdep.cpu.brand_string").into()),
            ("Memory", format!("{} GB", memory >> 30).into()),
        ]
    });
    match &*row.path {
        COMPUTER => Some(COMPUTER_FIELDS.to_vec()),
        NETWORK | AIRDROP => Some(vec![("Kind", row.kind.clone())]),
        _ => None,
    }
}

pub(crate) fn computer_name() -> &'static str {
    static NAME: LazyLock<&'static str> = LazyLock::new(|| {
        let name = unsafe { SCDynamicStoreCopyComputerName(ptr::null(), ptr::null_mut()) };
        intern(
            &unsafe { Retained::from_raw(name) }.map_or_else(String::new, |name| name.to_string()),
        )
    });
    *NAME
}

/// A location's title, and the location Enclosing Folder goes up to.
pub(crate) fn place(id: &str) -> Option<(&'static str, Option<&'static str>)> {
    let say = |english: &str| intern(&finder_fs::words::say(english));
    Some(match id {
        COMPUTER => (computer_name(), None),
        NETWORK => (say("Network"), Some(COMPUTER)),
        AIRDROP => (say("AirDrop"), None),
        SHARED => (say("Shared"), None),
        _ => return None,
    })
}

thread_local! {
    /// Finder's Network list as sharingd last sent it, and the channel it sends the next on.
    static NETWORKED: RefCell<(Option<Receiver<Vec<Computer>>>, Vec<Computer>)> =
        const { RefCell::new((None, Vec::new())) };
}

static NETWORK_CHANGED: Signal = Signal::new();

fn server_key(name: &str) -> String {
    format!("{NETWORK}/{}", name.replace('/', ":"))
}

pub(crate) fn server(key: &str) -> Option<Computer> {
    NETWORKED.with_borrow(|(_, listed)| {
        listed
            .iter()
            .find(|computer| server_key(&computer.name) == key)
            .cloned()
    })
}

fn location_row(
    path: &'static str,
    name: SharedString,
    kind: SharedString,
    uti: &'static str,
    opens: Option<&'static str>,
    artwork: FileType<'static>,
) -> Arc<Row> {
    Arc::new(Row {
        display: name.clone(),
        name,
        path: path.into(),
        size: "--".into(),
        bytes: 0,
        modified_at: f64::NAN,
        created_at: f64::NAN,
        added_at: None,
        kind,
        uti: uti.into(),
        face: Face::Volume,
        hidden: false,
        opens,
        discloses: false,
        dataless: false,
        readable: true,
        package: false,
        artwork: Some(artwork),
        extra: None,
    })
}

fn computer_rows() -> Vec<Arc<Row>> {
    let mut rows: Vec<Arc<Row>> = data::volume_roots()
        .into_iter()
        .filter_map(data::path_line)
        .map(|line| line.row)
        .collect();
    let network = place(NETWORK).map_or(NETWORK, |(title, _)| title);
    rows.push(location_row(
        NETWORK,
        network.into(),
        network.into(),
        "",
        Some(NETWORK),
        FileType::Named("NSNetwork"),
    ));
    rows
}

fn network_rows() -> Vec<Arc<Row>> {
    NETWORKED.with_borrow(|(_, listed)| {
        listed
            .iter()
            .map(|computer| {
                let (uti, kind) = finder_fs::servers::model_type(computer.model.as_deref())
                    .map(|(uti, kind)| (intern(&uti), kind))
                    .unwrap_or(("com.apple.mac", String::new()));
                location_row(
                    intern(&server_key(&computer.name)),
                    computer.name.clone().into(),
                    kind.into(),
                    uti,
                    None,
                    FileType::Uti(uti),
                )
            })
            .collect()
    })
}

fn shared_rows() -> Vec<Arc<Row>> {
    let Ok(found) = std::process::Command::new("/usr/bin/mdfind")
        .arg("kMDItemIsShared == 1")
        .output()
    else {
        return Vec::new();
    };
    let found = String::from_utf8_lossy(&found.stdout);
    data::rows_at(found.lines().take(HITS), crate::index())
}

fn rows_of(id: &'static str) -> Vec<Arc<Row>> {
    match id {
        COMPUTER => computer_rows(),
        SHARED => shared_rows(),
        _ => Vec::new(),
    }
}

fn follow_network(cx: &mut App) {
    if NETWORKED.with_borrow(|(following, _)| following.is_some()) {
        return;
    }
    let (sent, lists) = channel();
    NETWORKED.with_borrow_mut(|(following, _)| *following = Some(lists));
    std::thread::spawn(move || {
        finder_fs::servers::network(|list| {
            let open = sent.send(list).is_ok();
            NETWORK_CHANGED.ring();
            open
        })
    });
    cx.spawn(async move |cx| {
        let mut seen = 0;
        loop {
            seen = NETWORK_CHANGED.after(seen).await;
            NETWORKED.with_borrow_mut(|(following, listed)| {
                if let Some(latest) = following.iter().flat_map(|lists| lists.try_iter()).last() {
                    *listed = latest;
                }
            });
            let windows = cx.update(|cx| {
                cx.windows()
                    .into_iter()
                    .filter_map(|window| window.downcast::<FinderDemo>())
                    .collect::<Vec<_>>()
            });
            for window in windows {
                window
                    .update(cx, |this, _, cx| {
                        if this.folders.has(NETWORK) {
                            this.read_location(NETWORK, cx);
                        }
                    })
                    .ok();
            }
        }
    })
    .detach();
}

impl FinderDemo {
    pub(crate) fn read_location(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !self.folders.start(id) {
            return;
        }
        if !self.folders.has(id) {
            self.folders.deliver(id, Vec::new(), true);
        }
        let sorted_by = (self.sort, self.ascending);
        if id == NETWORK {
            follow_network(cx);
            let mut rows = network_rows();
            crate::read::sort_rows(&mut rows, sorted_by, false);
            self.deliver(id, Some(rows), None, true, sorted_by, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move {
                    let mut rows = rows_of(id);
                    crate::read::sort_rows(&mut rows, sorted_by, false);
                    rows
                })
                .await;
            this.update(cx, |this, cx| {
                this.deliver(id, Some(rows), None, true, sorted_by, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn connect_server(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = server(key).and_then(|computer| computer.url()) else {
            return;
        };
        if !url.starts_with("vnc:") {
            self.connect(url, window, cx);
        } else if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    pub(crate) fn airdrop_paths(&mut self, paths: &[PathBuf]) {
        let items: Vec<native_gpui::ShareItem> = paths
            .iter()
            .filter_map(|path| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                url.absoluteString()
                    .map(|url| native_gpui::ShareItem::Url(url.to_string()))
            })
            .collect();
        if !items.is_empty() && !native_gpui::airdrop(&items) {
            self.complain("AirDrop isn’t available for the selected items.");
        }
    }
}
