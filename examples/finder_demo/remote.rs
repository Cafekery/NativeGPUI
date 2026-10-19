mod jobs;
mod preview;
mod promise;

pub(crate) use self::jobs::{Ended, Jobs, cached};
pub(crate) use self::preview::{cached_copy, prune_fetches};
pub(crate) use self::promise::{cache, file_provider, prefetch};

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use block2::RcBlock;
use finder_fs::entry::{Entry as Local, EntryKind};
use finder_fs::ops::Op;
use finder_sftp::askpass::{self, Prompt};
use finder_sftp::{Address, Attrs, Entry, Error, Kind, Server, keychain};
use gpui::{AppContext as _, Context, Window};
use native_gpui::tokens::FileType;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSButton,
    NSControlStateValueOn, NSMenu, NSMenuItem, NSSecureTextField, NSView,
};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize, NSString};
use parking_lot::RwLock;

use crate::app::FinderDemo;
use crate::chrome::connect::{close, complain, remember, set_connecting};
use crate::data::{Line, Row, byte_string, face_of, intern, join, reference_seconds};
use crate::locations::COMPUTER;
use crate::ops::After;
use crate::quicklook::Look;
use crate::read::{Trouble, sort_rows};

const SCHEME: &str = "sftp://";
/// The askpass helper prints this to ssh's stderr, which `Error::Connect` carries back.
const CANCELLED: &str = "Found'er: cancelled";
const CANCEL: u8 = b'x';

#[derive(Clone)]
pub(crate) struct Connected {
    pub(crate) prefix: &'static str,
    pub(crate) path: &'static str,
    pub(crate) host: &'static str,
    server: Server,
    uid: Option<u32>,
}

static SERVERS: RwLock<Vec<Connected>> = RwLock::new(Vec::new());

pub(crate) enum Then {
    Open(Vec<PathBuf>),
    QuickLook(Look),
    Promise(RcBlock<dyn Fn(*mut NSError)>),
    MoveIn(Vec<PathBuf>),
}

pub(crate) fn is_remote(id: &str) -> bool {
    id.starts_with(SCHEME)
}

pub(crate) fn is_prefix(text: &str) -> bool {
    text.strip_prefix(SCHEME)
        .is_some_and(|rest| !rest.contains('/'))
}

fn split(id: &str) -> (&str, &str) {
    let at = id[SCHEME.len().min(id.len())..]
        .find('/')
        .map_or(id.len(), |at| SCHEME.len() + at);
    id.split_at(at)
}

pub(crate) fn same_server(item: &Path, into: &Path) -> bool {
    fn prefix(path: &Path) -> Option<&str> {
        Some(split(path.to_str().filter(|id| is_remote(id))?).0)
    }
    prefix(item).is_some_and(|item| prefix(into) == Some(item))
}

fn root_of(prefix: &str) -> &'static str {
    intern(&format!("{prefix}/"))
}

pub(crate) fn is_root(id: &str) -> bool {
    SERVERS
        .read()
        .iter()
        .any(|server| id == root_of(server.prefix) || id == server.path)
}

pub(crate) fn servers() -> Vec<Connected> {
    SERVERS.read().clone()
}

fn connected(id: &str) -> Option<Connected> {
    let prefix = split(id).0;
    SERVERS
        .read()
        .iter()
        .find(|server| server.prefix == prefix)
        .cloned()
}

pub(crate) fn host(id: &str) -> String {
    let prefix = split(id).0;
    Address::parse(prefix).map_or_else(|| prefix.into(), |address| address.host)
}

pub(crate) fn parent(id: &str) -> &'static str {
    let (prefix, path) = split(id);
    match path.rsplit_once('/') {
        _ if path.len() <= 1 => COMPUTER,
        Some(("", _)) => root_of(prefix),
        Some((up, _)) => intern(&format!("{prefix}{up}")),
        None => COMPUTER,
    }
}

pub(crate) fn crumbs(id: &str) -> Vec<(&'static str, &'static str)> {
    let (prefix, path) = split(id);
    let mut at = prefix.to_string();
    let parts = path.split('/').filter(|part| !part.is_empty());
    let mut crumbs = vec![(intern(&host(prefix)), root_of(prefix))];
    crumbs.extend(parts.map(|part| {
        at.push('/');
        at.push_str(part);
        (intern(part), intern(&at))
    }));
    crumbs
}

pub(crate) fn title(id: &str) -> &'static str {
    crumbs(id).pop().map_or("", |(title, _)| title)
}

pub(crate) fn address(text: &str) -> Option<Address> {
    let scheme = text.trim().get(..SCHEME.len())?;
    scheme
        .eq_ignore_ascii_case(SCHEME)
        .then(|| Address::parse(text))?
}

pub(crate) fn folder_at(text: &str) -> Option<&'static str> {
    let id = text.trim().trim_end_matches('/');
    let id = if is_prefix(id) {
        root_of(id)
    } else {
        intern(id)
    };
    connected(id).map(|_| id)
}

pub(crate) fn artwork(id: &str) -> FileType<'static> {
    if id.strip_suffix('/').is_some_and(is_prefix) {
        FileType::Named("NSNetwork")
    } else {
        FileType::Folder
    }
}

pub(crate) fn computer_rows() -> Vec<Arc<Row>> {
    servers()
        .into_iter()
        .map(|server| {
            crate::locations::location_row(
                server.path,
                server.host.into(),
                finder_fs::words::say("Server").into_owned().into(),
                "",
                Some(server.path),
                FileType::Named("NSNetwork"),
            )
        })
        .collect()
}

pub(crate) struct Stat {
    pub(crate) modified: f64,
    pub(crate) mode: Option<u32>,
    pub(crate) mine: Option<bool>,
}

pub(crate) fn stat(id: &str) -> Option<Stat> {
    let server = connected(id)?;
    let attrs = server.server.stat(split(id).1).ok()?;
    Some(Stat {
        modified: attrs.mtime.map_or(f64::NAN, seconds),
        mode: attrs.permissions.map(|mode| mode & 0o7777),
        mine: server.uid.zip(attrs.uid).map(|(me, owner)| me == owner),
    })
}

fn seconds(mtime: u32) -> f64 {
    reference_seconds(UNIX_EPOCH + Duration::from_secs(mtime.into()))
}

pub(crate) fn line(id: &'static str) -> Line {
    let kind = finder_fs::words::say("Folder").into_owned().into();
    let row = crate::locations::location_row(id, title(id).into(), kind, "", Some(id), artwork(id));
    Line {
        key: row.path.clone(),
        row,
        depth: 0,
    }
}

fn connect(address: Address, askpass: Option<&Path>) -> Result<&'static str, Error> {
    let prefix = intern(&address.root());
    let known = connected(prefix);
    let server = match &known {
        Some(known) => known.server.clone(),
        None => Server::connect(&address, askpass)?,
    };
    let path = match address.path.trim_end_matches('/') {
        "" if address.path.is_empty() => server.home(),
        "" => "/",
        path => path,
    };
    let path = intern(&format!("{prefix}{path}"));
    if known.is_none() {
        let uid = server.stat(server.home()).ok().and_then(|attrs| attrs.uid);
        SERVERS.write().push(Connected {
            prefix,
            path,
            host: intern(&address.host),
            server,
            uid,
        });
    }
    Ok(path)
}

fn rows(server: &Server, dir: &'static str) -> Result<Vec<Arc<Row>>, Error> {
    let path = split(dir).1;
    Ok(server
        .list(path)?
        .into_iter()
        .map(|entry| {
            let followed = (entry.attrs.kind() == Kind::Symlink)
                .then(|| server.stat_follow(&join(path, &entry.name)).ok());
            row(dir, entry, followed)
        })
        .collect())
}

pub(crate) fn find(
    dir: &'static str,
    admits: &dyn Fn(&Row) -> bool,
    limit: usize,
    stale: &dyn Fn() -> bool,
) -> Option<Vec<Arc<Row>>> {
    let server = connected(dir)?.server;
    let mut found = Vec::new();
    let mut seen = HashSet::from([dir]);
    let mut pending = VecDeque::from([dir]);
    while let Some(dir) = pending.pop_front() {
        if stale() {
            return None;
        }
        let Ok(rows) = rows(&server, dir) else {
            continue;
        };
        for row in rows {
            if let Some(child) = row.opens.filter(|_| !row.hidden) {
                if seen.insert(child) {
                    pending.push_back(child);
                }
            }
            if admits(&row) {
                found.push(row);
                if found.len() >= limit {
                    return Some(found);
                }
            }
        }
    }
    Some(found)
}

fn row(dir: &'static str, entry: Entry, followed: Option<Option<Attrs>>) -> Arc<Row> {
    let attrs = followed.flatten().unwrap_or(entry.attrs);
    let folder = attrs.kind() == Kind::Dir;
    let modified = UNIX_EPOCH + Duration::from_secs(attrs.mtime.unwrap_or(0).into());
    let local = Local {
        kind: if folder {
            EntryKind::Directory
        } else if followed.is_some() {
            EntryKind::Symlink
        } else {
            EntryKind::File
        },
        size: attrs.size.unwrap_or(0),
        modified,
        created: modified,
        added: None,
        hidden: entry.name.starts_with('.'),
        package: false,
        ext_hidden: false,
        alias: false,
        custom_icon: false,
        dataless: false,
        readable: true,
        file_id: 0,
        children: None,
        name: entry.name,
    };
    let kind = finder_fs::kind::kind_of(&local);
    let path = join(dir, &local.name);
    let bytes = attrs.size.filter(|_| !folder);
    let modified_at = attrs.mtime.map_or(f64::NAN, seconds);
    let artwork = match local.extension() {
        _ if folder => FileType::Folder,
        Some(extension) => FileType::Extension(intern(&extension.to_ascii_lowercase())),
        None => FileType::Uti(intern(&kind.uti)),
    };
    Arc::new(Row {
        display: local.name.clone().into(),
        name: local.name.into(),
        size: bytes.map_or("--".into(), |bytes| byte_string(bytes).into()),
        bytes: bytes.unwrap_or(0),
        modified_at,
        created_at: modified_at,
        added_at: None,
        kind: (&kind.label).into(),
        uti: (&kind.uti).into(),
        face: face_of(&kind.uti),
        hidden: local.hidden,
        opens: folder.then(|| intern(&path)),
        discloses: folder,
        dataless: false,
        readable: true,
        package: false,
        artwork: Some(artwork),
        extra: None,
        app: None,
        path: path.into(),
    })
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| line.chars().any(char::is_alphanumeric))
        .unwrap_or_default()
}

impl FinderDemo {
    pub(crate) fn connect_sftp(
        &mut self,
        address: Address,
        typed: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        let host = address.host.clone();
        let askpass = std::env::current_exe().ok();
        cx.spawn(async move |this, cx| {
            set_connecting(true, cx);
            let connected = cx
                .background_executor()
                .spawn(async move { connect(address, askpass.as_deref()) })
                .await;
            set_connecting(false, cx);
            match connected {
                Ok(path) => {
                    remember(&typed);
                    close(cx);
                    cx.update(crate::chrome::sidebar::reload_sidebars);
                    cx.update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| this.go(path, window, cx))
                    })
                    .ok();
                }
                Err(Error::Connect(text)) if text.contains(CANCELLED) => {}
                Err(error) => complain(
                    &format!("Unable to connect to the server \u{201c}{host}\u{201d}."),
                    first_line(&error.to_string()),
                    cx,
                ),
            }
        })
        .detach();
    }

    pub(crate) fn read_remote(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if !self.folders.start(id) {
            return;
        }
        let fresh = !self.folders.has(id);
        if fresh {
            self.folders.deliver(id, Vec::new(), true);
        }
        let order = self.order();
        let plain = self.plain();
        let server = connected(id).map(|connected| connected.server);
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let mut rows = rows(&server.ok_or(Error::Disconnected)?, id)?;
                    sort_rows(&mut rows, order, false, false);
                    let lines =
                        plain.map(|hidden| (hidden, crate::listing::lines_of(&rows, hidden)));
                    Ok((rows, lines))
                })
                .await;
            this.update(cx, |this, cx| match read {
                Ok((rows, lines)) => this.deliver(id, Some(rows), lines, true, order, cx),
                Err(error) => {
                    let dropped = matches!(error, Error::Disconnected) && this.eject_remote(id, cx);
                    let kind = if error.is_permission() {
                        io::ErrorKind::PermissionDenied
                    } else {
                        io::ErrorKind::Other
                    };
                    let error = io::Error::new(kind, error.to_string());
                    this.troubled(id, Trouble::Refused(error), fresh || dropped, order, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn eject_remote(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let prefix = split(id).0;
        let gone = {
            let mut servers = SERVERS.write();
            let at = servers.iter().position(|server| server.prefix == prefix);
            at.map(|at| servers.remove(at))
        };
        let Some(gone) = gone else {
            return false;
        };
        gone.server.disconnect();
        let root = root_of(prefix);
        cx.defer(move |cx| {
            crate::settings::each_window(cx, |this, cx| {
                for (id, _) in this.folders.held() {
                    if id.starts_with(root) {
                        this.folders.forget(id);
                    }
                }
                this.folder_gone(root, COMPUTER, cx);
                if this.folders.has(COMPUTER) {
                    this.read_location(COMPUTER, cx);
                }
            });
            crate::chrome::sidebar::reload_sidebars(cx);
        });
        true
    }

    pub(crate) fn remote_row(&self, dir: &'static str, key: &str) -> Option<Arc<Row>> {
        (self.folders.get(dir).into_iter())
            .flat_map(|folder| folder.rows.iter())
            .chain(&self.hits)
            .find(|row| row.path.as_ref() == key)
            .cloned()
    }

    pub(crate) fn remote_artwork(&self, path: &Path) -> Option<FileType<'static>> {
        let id = path.to_str().filter(|id| is_remote(id))?;
        let row = crate::data::split_key(id).and_then(|(dir, _)| self.remote_row(dir, id));
        Some(match row.and_then(|row| row.artwork) {
            Some(artwork) => artwork,
            None => match path.extension() {
                Some(ext) => FileType::Extension(intern(&ext.to_string_lossy().to_lowercase())),
                None => FileType::Folder,
            },
        })
    }

    pub(crate) fn fetch(
        &mut self,
        ids: Vec<PathBuf>,
        then: impl FnOnce(Vec<PathBuf>) -> Then,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let pairs: Vec<(PathBuf, PathBuf)> = (ids.into_iter())
            .filter_map(|id| Some((id.clone(), jobs::cached(id.to_str()?)?)))
            .collect();
        let into = pairs.first().and_then(|(_, to)| to.parent())?;
        let op = Op::Copy {
            items: pairs.iter().map(|(id, _)| id.clone()).collect(),
            into: into.to_path_buf(),
        };
        let then = then(pairs.iter().map(|(_, to)| to.clone()).collect());
        Some(self.submit_remote(op, pairs, Some(then), After::Nothing, cx))
    }

    pub(crate) fn then(&mut self, then: Then, clean: bool, cx: &mut Context<Self>) {
        match then {
            Then::Open(files) if clean => self.open_files(files, cx),
            Then::QuickLook(look) if clean => {
                let window = look.window;
                cx.spawn(async move |this, cx| {
                    cx.update_window(window, |_, window, cx| {
                        this.update(cx, |this, cx| this.looked(look, window, cx))
                    })
                    .ok();
                })
                .detach();
            }
            Then::Promise(done) => promise::promised(&done, clean),
            _ => {}
        }
    }

    pub(crate) fn reread_remote(&mut self, cx: &mut Context<Self>) {
        for (id, _) in self.folders.held() {
            if !is_remote(id) {
                continue;
            }
            if self.folders.shown(id) {
                self.read_folder(id, cx);
            } else {
                self.folders.stale(id);
            }
        }
    }
}

/// The app re-run by ssh as `SSH_ASKPASS`: 0 with the answer on stdout, 1 to refuse.
pub(crate) fn askpass(prompt: &str) -> i32 {
    let var = |name| std::env::var(name).unwrap_or_default();
    let host = var(askpass::HOST);
    match askpass::classify(prompt) {
        Prompt::Password => password(&host, &var(askpass::USER), &var(askpass::ATTEMPT)),
        Prompt::HostKey { text } => {
            let fingerprint = text.lines().find(|line| line.contains("fingerprint is"));
            let message =
                format!("Found'er can\u{2019}t verify the identity of \u{201c}{host}\u{201d}.");
            if !ask(&message, fingerprint.unwrap_or(&text), None) {
                eprintln!("{CANCELLED}");
                return 1;
            }
            println!("yes");
            0
        }
        Prompt::Other(_) => 1,
    }
}

fn password(host: &str, user: &str, attempt: &str) -> i32 {
    let seen = fs::read(attempt).unwrap_or_default();
    if seen.contains(&CANCEL) {
        return 1;
    }
    let mark = |byte: u8| {
        let file = fs::OpenOptions::new().append(true).open(attempt);
        file.and_then(|mut file| file.write_all(&[byte])).ok();
    };
    mark(b'.');
    let cache = std::env::var(askpass::CACHE).unwrap_or_default();
    if seen.is_empty() {
        let cached = fs::read_to_string(&cache)
            .ok()
            .filter(|known| !known.is_empty());
        let known = cached.or_else(|| {
            let found = keychain::find(host, user)?;
            let _ = fs::write(&cache, &found);
            Some(found)
        });
        if let Some(known) = known {
            println!("{known}");
            return 0;
        }
    } else {
        keychain::forget(host, user);
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return 1;
    };
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame(0., 0., 260., 50.));
    let field =
        NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), frame(0., 28., 260., 22.));
    let keep = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Remember this password in my keychain"),
            None,
            None,
            mtm,
        )
    };
    keep.setFrame(frame(0., 0., 260., 18.));
    keep.setState(NSControlStateValueOn);
    view.addSubview(&field);
    view.addSubview(&keep);
    let message = format!("Enter your name and password for the server \u{201c}{host}\u{201d}.");
    let detail = format!("Connecting as {user}.");
    if !ask(&message, &detail, Some((&view, &field))) {
        mark(CANCEL);
        eprintln!("{CANCELLED}");
        return 1;
    }
    let typed = field.stringValue().to_string();
    let _ = fs::write(&cache, &typed);
    if keep.state() == NSControlStateValueOn {
        keychain::store(host, user, &typed);
    }
    println!("{typed}");
    0
}

// A helper has no menu bar, and without an Edit menu Cmd-V reaches nothing.
fn edit_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let edit = NSMenu::new(mtm);
    let items = [
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ];
    for (title, action, key) in items {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        edit.addItem(&item);
    }
    let bar = NSMenu::new(mtm);
    let item = NSMenuItem::new(mtm);
    item.setSubmenu(Some(&edit));
    bar.addItem(&item);
    bar
}

fn frame(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn ask(message: &str, detail: &str, accessory: Option<(&NSView, &NSView)>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    #[allow(deprecated)]
    // cooperative activate() loses to the parent app; this is what makes a helper's alert key
    app.activateIgnoringOtherApps(true);
    app.setMainMenu(Some(&edit_menu(mtm)));
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(detail.trim()));
    alert.addButtonWithTitle(&NSString::from_str("Connect"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    if let Some((view, focus)) = accessory {
        alert.setAccessoryView(Some(view));
        alert.layout();
        alert.window().setInitialFirstResponder(Some(focus));
    }
    alert.runModal() == NSAlertFirstButtonReturn
}
