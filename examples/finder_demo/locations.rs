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
    let read = |value: *mut c_void, size: &mut usize| unsafe {
        sysctlbyname(name.as_ptr(), value, size, ptr::null(), 0) == 0
    };
    let mut size = 0;
    if !read(ptr::null_mut(), &mut size) {
        return None;
    }
    let mut value = vec![0; size];
    read(value.as_mut_ptr().cast(), &mut size).then(|| {
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

fn computer_type(model: Option<&str>) -> (&'static str, String) {
    finder_fs::servers::model_type(model)
        .map(|(uti, kind)| (intern(&uti), kind))
        .unwrap_or(("com.apple.mac", String::new()))
}

pub(crate) fn artwork(id: &str) -> Option<FileType<'static>> {
    static THIS_MAC: LazyLock<&'static str> =
        LazyLock::new(|| computer_type(Some(&sysctl_text(c"hw.model"))).0);
    match id {
        COMPUTER => Some(FileType::Uti(*THIS_MAC)),
        NETWORK => Some(FileType::Named("NSNetwork")),
        AIRDROP => Some(FileType::Path(AIRDROP_APP)),
        _ => None,
    }
}

pub(crate) fn info_line(id: &'static str) -> Option<Line> {
    let kind = match id {
        NETWORK => "Neighborhood",
        AIRDROP => "AirDrop",
        _ => "",
    };
    let row = place_row(id, kind)?;
    Some(Line {
        key: row.path.clone(),
        row,
        depth: 0,
    })
}

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

pub(crate) fn computer_symbol() -> String {
    finder_fs::servers::symbol(Some(&sysctl_text(c"hw.model")))
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

pub(crate) fn location_row(
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
        app: None,
    })
}

fn place_row(id: &'static str, kind: &str) -> Option<Arc<Row>> {
    let (title, _) = place(id)?;
    let kind = finder_fs::words::say(kind).into_owned().into();
    Some(location_row(
        id,
        title.into(),
        kind,
        "",
        Some(id),
        artwork(id)?,
    ))
}

fn computer_rows() -> Vec<Arc<Row>> {
    data::volume_roots()
        .into_iter()
        .filter_map(data::path_line)
        .map(|line| line.row)
        .chain(crate::remote::computer_rows())
        .chain(place_row(NETWORK, "Network"))
        .collect()
}

fn network_rows() -> Vec<Arc<Row>> {
    NETWORKED.with_borrow(|(_, listed)| {
        listed
            .iter()
            .map(|computer| {
                let (uti, kind) = computer_type(computer.model.as_deref());
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
    let mut rows = data::rows_at(found.lines().take(HITS));
    data::read_sharers(&mut rows);
    rows
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
    pub(crate) fn airdrop_shown(&self) -> bool {
        self.here() == AIRDROP && self.tag_listing.is_none() && !self.in_search()
    }

    pub(crate) fn read_location(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !self.folders.start(id) {
            return;
        }
        if !self.folders.has(id) {
            self.folders.deliver(id, Vec::new(), true);
        }
        let order = self.order();
        let servers = (id == NETWORK).then(|| {
            follow_network(cx);
            network_rows()
        });
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move {
                    let mut rows = servers.unwrap_or_else(|| rows_of(id));
                    crate::read::sort_rows(&mut rows, order, false, true);
                    rows
                })
                .await;
            this.update(cx, |this, cx| {
                this.deliver(id, Some(rows), None, true, order, cx)
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
        if url.starts_with("vnc:") {
            cx.open_url(&url);
        } else {
            self.connect(url, window, cx);
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
