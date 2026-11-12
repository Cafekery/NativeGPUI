use std::collections::VecDeque;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use finder_fs::ops::Op;
use finder_fs::ops::transfer::{Current, End, Failure, Progress};
use finder_sftp::{Attrs, Error, Kind, Server, safe_name};
use parking_lot::{Condvar, Mutex};

use super::{Then, connected, split};

const NAME_MAX: usize = 255;
const SPEED_OVER: Duration = Duration::from_secs(2);

static NEXT: AtomicU64 = AtomicU64::new(u64::MAX);

#[derive(Default)]
struct State {
    progress: Mutex<Progress>,
    wake: Condvar,
    cancelled: AtomicBool,
}

pub(crate) struct Ended {
    pub(crate) results: Vec<PathBuf>,
    pub(crate) failures: Vec<Failure>,
    pub(crate) end: End,
    pub(crate) taken: Option<String>,
    pub(crate) dropped: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct Jobs {
    running: Vec<(u64, Arc<State>, Option<Then>)>,
    ended: Arc<Mutex<Vec<(u64, Ended)>>>,
}

impl Jobs {
    pub(crate) fn start(
        &mut self,
        op: Op,
        fetch: Vec<(PathBuf, PathBuf)>,
        then: Option<Then>,
    ) -> u64 {
        let id = NEXT.fetch_sub(1, Ordering::Relaxed);
        let state = Arc::new(State::default());
        self.running.push((id, state.clone(), then));
        let ended = self.ended.clone();
        std::thread::spawn(move || {
            let mut run = Run {
                state,
                samples: VecDeque::new(),
                base: 0,
                ended: Ended {
                    results: Vec::new(),
                    failures: Vec::new(),
                    end: End::Finished,
                    taken: None,
                    dropped: None,
                },
            };
            run.work(op, fetch);
            ended.lock().push((id, run.ended));
            crate::read::EVENTS.ring();
        });
        id
    }

    fn state(&self, id: u64) -> Option<&State> {
        let found = self.running.iter().find(|(job, ..)| *job == id);
        found.map(|(_, state, _)| &**state)
    }

    pub(crate) fn progress(&self, id: u64) -> Option<Progress> {
        Some(self.state(id)?.progress.lock().clone())
    }

    pub(crate) fn pause(&self, id: u64) {
        if let Some(state) = self.state(id) {
            state.progress.lock().paused = true;
        }
    }

    pub(crate) fn resume(&self, id: u64) {
        if let Some(state) = self.state(id) {
            state.progress.lock().paused = false;
            state.wake.notify_all();
        }
    }

    pub(crate) fn cancel(&self, id: u64) {
        if let Some(state) = self.state(id) {
            // Held so `tick` cannot check `cancelled` and park between the store and the notify.
            let _held = state.progress.lock();
            state.cancelled.store(true, Ordering::Relaxed);
            state.wake.notify_all();
        }
    }

    pub(crate) fn ended(&mut self) -> Vec<(u64, Ended, Option<Then>)> {
        let ended = std::mem::take(&mut *self.ended.lock());
        (ended.into_iter())
            .filter_map(|(id, ended)| {
                let at = self.running.iter().position(|(job, ..)| *job == id)?;
                Some((id, ended, self.running.remove(at).2))
            })
            .collect()
    }
}

pub(crate) fn cached(id: &str) -> Option<PathBuf> {
    let address = connected(id)?.server.address().clone();
    let host = match address.port {
        Some(port) if port != 22 => format!("{}_{port}", address.host),
        _ => address.host,
    };
    let path = split(id).1.trim_start_matches('/');
    if !confined(path) {
        return None;
    }
    Some(cache_root().join(host).join(path))
}

fn confined(path: &str) -> bool {
    !path.contains('\0') && path.split('/').all(|part| part != "..")
}

pub(super) fn cache_root() -> PathBuf {
    crate::home().join("Library/Caches/dev.cafekery.founder/sftp")
}

#[derive(Clone)]
enum Place {
    Here(PathBuf),
    There {
        server: Server,
        prefix: &'static str,
        path: String,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Node {
    File { size: u64, mtime: Option<u32> },
    Dir,
    Link,
    Other,
}

impl Node {
    fn of(attrs: &Attrs) -> Node {
        match attrs.kind() {
            Kind::Dir => Node::Dir,
            Kind::File => Node::File {
                size: attrs.size.unwrap_or(0),
                mtime: attrs.mtime,
            },
            _ => Node::Other,
        }
    }
}

fn place(path: &Path) -> Result<Place, Error> {
    let id = path.to_string_lossy();
    if !super::is_remote(&id) {
        return Ok(Place::Here(path.to_path_buf()));
    }
    let server = connected(&id).ok_or(Error::Disconnected)?;
    Ok(Place::There {
        server: server.server,
        prefix: server.prefix,
        path: split(&id).1.to_owned(),
    })
}

fn once<T>(mut run: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    match run() {
        Err(Error::Disconnected) => run(),
        found => found,
    }
}

fn absent<T>(found: Result<T, Error>) -> Result<Option<T>, Error> {
    match found {
        Ok(found) => Ok(Some(found)),
        Err(Error::Io(error)) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

impl Place {
    fn id(&self) -> PathBuf {
        match self {
            Place::Here(path) => path.clone(),
            Place::There { prefix, path, .. } => PathBuf::from(format!("{prefix}{path}")),
        }
    }

    fn name(&self) -> String {
        match self {
            Place::Here(path) => finder_fs::name::of_path(path),
            Place::There { path, .. } => path.rsplit('/').next().unwrap_or_default().to_owned(),
        }
    }

    fn join(&self, name: &str) -> Place {
        match self {
            Place::Here(path) => Place::Here(path.join(name)),
            Place::There {
                server,
                prefix,
                path,
            } => Place::There {
                server: server.clone(),
                prefix,
                path: crate::data::join(path, name),
            },
        }
    }

    fn parent(&self) -> Place {
        match self {
            Place::Here(path) => Place::Here(path.parent().unwrap_or(path).to_path_buf()),
            Place::There {
                server,
                prefix,
                path,
            } => Place::There {
                server: server.clone(),
                prefix,
                path: match path.rsplit_once('/') {
                    Some(("", _)) | None => "/".to_owned(),
                    Some((up, _)) => up.to_owned(),
                },
            },
        }
    }

    fn server(&self) -> Option<&'static str> {
        match self {
            Place::Here(_) => None,
            Place::There { prefix, .. } => Some(prefix),
        }
    }

    fn node(&self) -> Result<Option<Node>, Error> {
        once(|| self.probe())
    }

    fn probe(&self) -> Result<Option<Node>, Error> {
        match self {
            Place::Here(path) => {
                let Some(meta) = absent(fs::symlink_metadata(path).map_err(Error::from))? else {
                    return Ok(None);
                };
                let link = meta.is_symlink();
                Ok(Some(match fs::metadata(path) {
                    Ok(meta) if meta.is_file() => Node::File {
                        size: meta.len(),
                        mtime: meta.mtime().try_into().ok(),
                    },
                    Ok(meta) if meta.is_dir() && !link => Node::Dir,
                    _ if link => Node::Link,
                    _ => Node::Other,
                }))
            }
            Place::There { server, path, .. } => {
                let Some(mut attrs) = absent(server.stat(path))? else {
                    return Ok(None);
                };
                if attrs.kind() == Kind::Symlink {
                    match server.stat_follow(path) {
                        Ok(target) if target.kind() != Kind::Dir => attrs = target,
                        _ => return Ok(Some(Node::Link)),
                    }
                }
                Ok(Some(Node::of(&attrs)))
            }
        }
    }

    fn children(&self) -> Result<Vec<(String, Option<Node>)>, Error> {
        once(|| self.list())
    }

    /// A listed entry that is a plain file or folder needs no `lstat` of its own.
    fn list(&self) -> Result<Vec<(String, Option<Node>)>, Error> {
        match self {
            Place::Here(path) => fs::read_dir(path)?
                .map(|entry| Ok((entry?.file_name().to_string_lossy().into_owned(), None)))
                .collect(),
            Place::There { server, path, .. } => Ok(server
                .list(path)?
                .into_iter()
                .filter(|entry| safe_name(&entry.name))
                .map(|entry| {
                    let known = matches!(entry.attrs.kind(), Kind::Dir | Kind::File);
                    (entry.name, known.then(|| Node::of(&entry.attrs)))
                })
                .collect()),
        }
    }

    fn delete(&self, folder: bool) -> Result<(), Error> {
        match (self, folder) {
            (Place::Here(path), true) => Ok(fs::remove_dir(path)?),
            (Place::Here(path), false) => Ok(fs::remove_file(path)?),
            (Place::There { server, path, .. }, true) => server.rmdir(path),
            (Place::There { server, path, .. }, false) => server.remove(path),
        }
    }

    fn read_link(&self) -> Result<PathBuf, Error> {
        match self {
            Place::Here(path) => Ok(fs::read_link(path)?),
            Place::There { server, path, .. } => Ok(server.readlink(path)?.into()),
        }
    }

    fn symlink(&self, target: &Path) -> Result<(), Error> {
        match self {
            Place::Here(path) => Ok(std::os::unix::fs::symlink(target, path)?),
            Place::There { server, path, .. } => server.symlink(&target.to_string_lossy(), path),
        }
    }

    fn mkdir(&self) -> Result<(), Error> {
        let made = once(|| match self {
            Place::Here(path) => fs::create_dir(path).map_err(Error::from),
            Place::There { server, path, .. } => server.mkdir(path),
        });
        match made {
            Err(_) if self.node()? == Some(Node::Dir) => Ok(()),
            made => made,
        }
    }

    fn free(&self, name: &str, tag: &str) -> Result<Place, Error> {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
            _ => (name, None),
        };
        for n in 1.. {
            let tag = match n {
                1 => tag.to_owned(),
                n => format!("{tag} {n}"),
            };
            let at = self.join(&fitted(stem, &tag, ext));
            if at.node()?.is_none() {
                return Ok(at);
            }
        }
        unreachable!("an endless range")
    }
}

fn fitted(stem: &str, tag: &str, ext: Option<&str>) -> String {
    let room = NAME_MAX.saturating_sub(tag.len() + ext.map_or(0, |ext| ext.len() + 1));
    let mut end = stem.len().min(room);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    match ext {
        Some(ext) => format!("{}{tag}.{ext}", &stem[..end]),
        None => format!("{}{tag}", &stem[..end]),
    }
}

fn io_error(error: Error) -> io::Error {
    let kind = if error.is_permission() {
        ErrorKind::PermissionDenied
    } else if error.is_not_found() {
        ErrorKind::NotFound
    } else {
        ErrorKind::Other
    };
    match error {
        Error::Io(error) => error,
        error => io::Error::new(kind, error.to_string()),
    }
}

fn landing(
    items: &[PathBuf],
    into: impl Fn(&Place) -> Result<(Place, &'static str), Error>,
) -> Result<Vec<(Place, Place)>, Error> {
    (items.iter())
        .map(|item| {
            let from = place(item)?;
            let (dir, tag) = into(&from)?;
            let to = dir.free(&from.name(), tag)?;
            Ok((from, to))
        })
        .collect()
}

fn trash(from: &Place) -> Result<Place, Error> {
    let Place::There { server, prefix, .. } = from else {
        return Err(Error::Protocol("not on a server".into()));
    };
    let path = crate::data::join(server.home(), ".Trash");
    let trash = Place::There {
        server: server.clone(),
        prefix,
        path: path.clone(),
    };
    if trash.node()? != Some(Node::Dir) {
        trash.mkdir()?;
        server.set_permissions(&path, 0o700)?;
    }
    Ok(trash)
}

struct Step {
    top: usize,
    from: Place,
    to: Place,
    size: Option<u64>,
    link: bool,
}

struct Run {
    state: Arc<State>,
    samples: VecDeque<(Instant, u64)>,
    base: u64,
    ended: Ended,
}

impl Run {
    fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Relaxed)
    }

    fn failed(&mut self, from: &Place, to: &Place, error: Error) -> bool {
        if matches!(error, Error::Cancelled) {
            return false;
        }
        let lost = matches!(error, Error::Disconnected);
        self.state.progress.lock().failures += 1;
        self.ended.failures.push(Failure {
            source: from.id(),
            dest: to.id(),
            error: io_error(error),
        });
        !lost
    }

    fn work(&mut self, op: Op, fetch: Vec<(PathBuf, PathBuf)>) {
        let first = match &op {
            Op::Copy { items, .. }
            | Op::CopyExactly { items, .. }
            | Op::Move { items, .. }
            | Op::Duplicate(items)
            | Op::Trash(items) => items.first(),
            Op::Rename { path: item, .. } | Op::NewFolder { into: item, .. } => Some(item),
            _ => fetch.first().map(|(item, _)| item),
        };
        let first = first.cloned().unwrap_or_default();
        let planned = if fetch.is_empty() {
            self.plan(op)
        } else {
            (fetch.into_iter())
                .map(|(item, to)| Ok((place(&item)?, Place::Here(to))))
                .collect::<Result<Vec<_>, Error>>()
                .map(|pairs| self.fetch(pairs))
        };
        if let Err(error) = planned {
            let at = place(&first).unwrap_or(Place::Here(first));
            self.failed(&at, &at, error);
        }
        self.ended.end = if self.cancelled() {
            End::Cancelled
        } else if !self.ended.failures.is_empty() {
            End::Stopped
        } else {
            End::Finished
        };
    }

    fn plan(&mut self, op: Op) -> Result<(), Error> {
        match op {
            Op::Copy { items, into } | Op::CopyExactly { items, into } => {
                let into = place(&into)?;
                let pairs = landing(&items, |from| {
                    let same = from.parent().id() == into.id();
                    Ok((into.clone(), if same { " copy" } else { "" }))
                })?;
                self.carry(pairs, false);
            }
            Op::Duplicate(items) => {
                let pairs = landing(&items, |from| Ok((from.parent(), " copy")))?;
                self.carry(pairs, false);
            }
            Op::Move { items, into } => {
                let into = place(&into)?;
                let pairs = landing(&items, |_| Ok((into.clone(), "")))?;
                let server = into.server();
                if server.is_some() && pairs.iter().all(|(from, _)| from.server() == server) {
                    self.shift(pairs);
                } else {
                    self.carry(pairs, true);
                }
            }
            Op::Trash(items) => self.shift(landing(&items, |from| Ok((trash(from)?, "")))?),
            Op::Rename { path, name } => {
                finder_fs::ops::check_name(&name)?;
                let from = place(&path)?;
                let to = from.parent().join(&name);
                let case = from.name().to_lowercase() == name.to_lowercase();
                if !case && to.node()?.is_some() {
                    self.ended.taken = Some(name);
                    return Ok(());
                }
                self.shift(vec![(from, to)]);
            }
            Op::NewFolder { into, name } => {
                finder_fs::ops::check_name(&name)?;
                let to = place(&into)?.free(&name, "")?;
                to.mkdir()?;
                self.ended.results.push(to.id());
            }
            _ => {}
        }
        Ok(())
    }

    fn shift(&mut self, pairs: Vec<(Place, Place)>) {
        self.state.progress.lock().items_total = pairs.len() as u64;
        for (from, to) in pairs {
            if self.cancelled() {
                return;
            }
            let renamed = once(|| match (&from, &to) {
                (Place::There { server, path, .. }, Place::There { path: to_path, .. }) => {
                    server.rename(path, to_path)
                }
                _ => Err(Error::Protocol("not on a server".into())),
            });
            match renamed {
                Ok(()) => self.ended.results.push(to.id()),
                Err(error) => {
                    if !self.failed(&from, &to, error) {
                        return;
                    }
                }
            }
            self.state.progress.lock().items_done += 1;
        }
    }

    fn fetch(&mut self, pairs: Vec<(Place, Place)>) {
        let mut stale = Vec::new();
        for (from, to) in pairs {
            let fresh = match (from.node(), to.node()) {
                (Ok(Some(remote @ Node::File { .. })), Ok(Some(local))) => remote == local,
                _ => false,
            };
            if fresh {
                self.ended.results.push(to.id());
                continue;
            }
            if let Place::Here(path) = &to {
                let made = path.parent().map_or(Ok(()), fs::create_dir_all);
                if let Err(error) = made {
                    self.failed(&from, &to, error.into());
                    continue;
                }
            }
            stale.push((from, to));
        }
        self.carry(stale, false);
    }

    fn carry(&mut self, pairs: Vec<(Place, Place)>, remove: bool) {
        self.state.progress.lock().counting = true;
        let mut steps = Vec::new();
        let mut whole = vec![true; pairs.len()];
        for (top, (from, to)) in pairs.iter().enumerate() {
            if let Err(error) = self.walk(top, from, to, None, &mut steps) {
                whole[top] = false;
                if !self.failed(from, to, error) {
                    return;
                }
            }
        }
        self.state.progress.lock().counting = false;
        for step in &steps {
            if !whole[step.top] {
                continue;
            }
            if self.cancelled() {
                return;
            }
            match self.step(step) {
                Ok(()) => self.state.progress.lock().items_done += 1,
                Err(error) => {
                    whole[step.top] = false;
                    if !self.failed(&step.from, &step.to, error) {
                        return;
                    }
                }
            }
        }
        if remove {
            for step in steps.iter().rev().filter(|step| whole[step.top]) {
                let folder = step.size.is_none() && !step.link;
                if let Err(error) = once(|| step.from.delete(folder)) {
                    if !folder && !self.failed(&step.from, &step.to, error) {
                        return;
                    }
                }
            }
        }
        let landed = pairs.iter().zip(whole).filter(|(_, whole)| *whole);
        (self.ended.results).extend(landed.map(|((_, to), _)| to.id()));
    }

    fn walk(
        &mut self,
        top: usize,
        from: &Place,
        to: &Place,
        known: Option<Node>,
        steps: &mut Vec<Step>,
    ) -> Result<(), Error> {
        if self.cancelled() {
            return Err(Error::Cancelled);
        }
        let node = match known {
            Some(node) => node,
            None => from
                .node()?
                .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?,
        };
        let size = match node {
            Node::Dir | Node::Link => None,
            Node::File { size, .. } => Some(size),
            Node::Other => {
                let why = format!(
                    "The item \u{201c}{}\u{201d} can't be copied because it isn't a file or a folder.",
                    from.name()
                );
                let error = io::Error::new(ErrorKind::Unsupported, why);
                self.failed(from, to, error.into());
                return Ok(());
            }
        };
        {
            let mut progress = self.state.progress.lock();
            progress.items_total += 1;
            progress.bytes_total += size.unwrap_or(0);
        }
        steps.push(Step {
            top,
            from: from.clone(),
            to: to.clone(),
            size,
            link: node == Node::Link,
        });
        if node == Node::Dir {
            for (name, known) in from.children()? {
                self.walk(top, &from.join(&name), &to.join(&name), known, steps)?;
            }
        }
        Ok(())
    }

    fn step(&mut self, step: &Step) -> Result<(), Error> {
        if step.link {
            return once(|| step.to.symlink(&step.from.read_link()?));
        }
        let Some(size) = step.size else {
            return step.to.mkdir();
        };
        let path = step.from.id();
        let mut tick = |done: u64, _: u64| self.tick(&path, done, size);
        let copied = once(|| copy(step, size, &mut tick));
        self.base += size;
        self.state.progress.lock().bytes_done = self.base;
        copied
    }

    fn tick(&mut self, path: &Path, done: u64, total: u64) -> bool {
        let mut progress = self.state.progress.lock();
        while progress.paused && !self.cancelled() {
            self.state.wake.wait(&mut progress);
            self.samples.clear();
        }
        let now = Instant::now();
        let bytes = self.base + done.min(total);
        self.samples.push_back((now, bytes));
        while (self.samples.front()).is_some_and(|(at, _)| now - *at > SPEED_OVER) {
            self.samples.pop_front();
        }
        let speed = self.samples.front().map_or(0., |(at, first)| {
            let secs = (now - *at).as_secs_f64();
            if secs < 0.5 {
                0.
            } else {
                (bytes - first) as f64 / secs
            }
        });
        progress.bytes_done = bytes;
        progress.bytes_per_sec = speed;
        progress.eta = (speed >= 1.).then(|| {
            Duration::from_secs_f64(progress.bytes_total.saturating_sub(bytes) as f64 / speed)
        });
        progress.current = Some(Current {
            path: path.to_path_buf(),
            bytes_done: done,
            bytes_total: total,
        });
        !self.cancelled()
    }
}

fn copy(step: &Step, size: u64, tick: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), Error> {
    match (&step.from, &step.to) {
        (Place::Here(from), Place::There { server, path, .. }) => server.upload(from, path, tick),
        (Place::There { server, path, .. }, Place::Here(to)) => server.download(path, to, tick),
        (
            Place::There {
                server,
                prefix,
                path,
            },
            Place::There {
                server: into,
                prefix: there,
                path: to,
            },
        ) => (prefix == there)
            .then(|| server.copy_data(path, to))
            .flatten()
            .unwrap_or_else(|| relay(server, path, into, to, size, tick)),
        (Place::Here(from), Place::Here(to)) => fs::copy(from, to).map(drop).map_err(Error::from),
    }
}

fn relay(
    from: &Server,
    path: &str,
    into: &Server,
    to: &str,
    size: u64,
    tick: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<(), Error> {
    static RELAYED: AtomicU64 = AtomicU64::new(0);
    let dir = cache_root();
    fs::create_dir_all(&dir)?;
    let temp = dir.join(format!(
        ".relay {}",
        RELAYED.fetch_add(1, Ordering::Relaxed)
    ));
    let relayed = from
        .download(path, &temp, &mut |done, _| tick(done / 2, size))
        .and_then(|()| into.upload(&temp, to, &mut |done, _| tick((size + done) / 2, size)));
    let _ = fs::remove_file(&temp);
    relayed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_path_cannot_leave_its_folder() {
        assert!(confined("a/b/c.png"));
        assert!(confined("a/..b/.c"));
        assert!(!confined("a/../../etc"));
        assert!(!confined(".."));
        assert!(!confined("a/b\0"));
    }

    #[test]
    fn a_dropped_link_is_retried_once() {
        let mut calls = 0;
        let found = once(|| {
            calls += 1;
            if calls == 1 {
                Err(Error::Disconnected)
            } else {
                Ok(calls)
            }
        });
        assert!(matches!(found, Ok(2)));
        let mut calls = 0;
        let found: Result<(), Error> = once(|| {
            calls += 1;
            Err(Error::Disconnected)
        });
        assert!(matches!(found, Err(Error::Disconnected)) && calls == 2);
    }
}
