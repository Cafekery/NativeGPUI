use std::collections::VecDeque;
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::session::{Pending, Session};
use crate::wire::*;
use crate::{Address, Attrs, Entry, Error, Space, askpass};

/// Transfer sessions; slot 0, before them, only carries metadata calls.
pub(crate) const POOL: usize = 4;
const CHUNK: u32 = 256 << 10;
const READ_AHEAD: usize = 4;
const SSH: &str = "/usr/bin/ssh";

enum Spawn {
    Ssh {
        address: Address,
        askpass: Option<Askpass>,
    },
    Command(Box<dyn Fn() -> Command + Send + Sync>),
}

struct Askpass {
    helper: PathBuf,
    dir: PathBuf,
}

impl Askpass {
    fn new(helper: &Path) -> Result<Askpass, Error> {
        let dir =
            std::env::temp_dir().join(format!("founder-sftp-{}-{}", std::process::id(), next()));
        DirBuilder::new().mode(0o700).create(&dir)?;
        let askpass = Askpass {
            helper: helper.to_owned(),
            dir,
        };
        File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(askpass.dir.join("cache"))?;
        Ok(askpass)
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn next() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Spawn {
    fn session(&self) -> Result<(Arc<Session>, Pending), Error> {
        match self {
            Spawn::Ssh { address, askpass } => ssh(address, askpass.as_ref()),
            Spawn::Command(make) => Session::spawn(make(), None),
        }
    }
}

fn ssh(address: &Address, askpass: Option<&Askpass>) -> Result<(Arc<Session>, Pending), Error> {
    let mut command = Command::new(SSH);
    command.args([
        "-oServerAliveInterval=15",
        "-oServerAliveCountMax=3",
        "-oConnectTimeout=20",
        "-oLogLevel=ERROR",
    ]);
    if let Some(port) = address.port {
        command.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = &address.identity {
        command.arg("-i").arg(identity);
    }
    let mut attempt = None;
    if let Some(askpass) = askpass {
        let path = askpass.dir.join(format!("attempt-{}", next()));
        File::create_new(&path)?;
        command
            .arg("-oBatchMode=no")
            .env("SSH_ASKPASS", &askpass.helper)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(askpass::HOST, &address.host)
            .env(askpass::USER, login(address))
            .env(askpass::ATTEMPT, &path)
            .env(askpass::CACHE, askpass.dir.join("cache"));
        attempt = Some(path);
    } else {
        command.arg("-oBatchMode=yes");
    }
    let target = match &address.user {
        Some(user) => format!("{user}@{}", address.host),
        None => address.host.clone(),
    };
    command.arg("-s").arg(target).arg("sftp");
    Session::spawn(command, attempt)
}

/// The account ssh will log in as, which ~/.ssh/config may set.
fn login(address: &Address) -> String {
    if let Some(user) = &address.user {
        return user.clone();
    }
    let mut command = Command::new(SSH);
    command.arg("-G");
    if let Some(port) = address.port {
        command.arg("-p").arg(port.to_string());
    }
    let output = command.arg(&address.host).output();
    let text = output.map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    text.ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("user ").map(str::to_owned))
        })
        .unwrap_or_default()
}

struct Pool {
    closed: bool,
    sessions: [Option<Arc<Session>>; 1 + POOL],
}

pub(crate) struct Inner {
    address: Address,
    spawn: Spawn,
    home: String,
    extensions: Vec<String>,
    pub(crate) read_size: u32,
    pub(crate) write_size: u32,
    pool: Mutex<Pool>,
}

impl Inner {
    pub(crate) fn session(&self, slot: usize) -> Result<Arc<Session>, Error> {
        let (session, version) = {
            let mut pool = self.pool.lock();
            if pool.closed {
                return Err(Error::Disconnected);
            }
            if let Some(session) = pool.sessions[slot].as_ref().filter(|s| s.alive()) {
                return Ok(session.clone());
            }
            let (session, version) = self.spawn.session()?;
            pool.sessions[slot] = Some(session.clone());
            (session, version)
        };
        match session.hello(version) {
            Ok(_) => Ok(session),
            Err(_) if self.pool.lock().closed => Err(Error::Disconnected),
            Err(error) => Err(error),
        }
    }
}

pub(crate) struct Handle {
    pub(crate) session: Arc<Session>,
    pub(crate) id: Vec<u8>,
}

impl Handle {
    pub(crate) fn open(session: &Arc<Session>, path: &str, flags: u32) -> Result<Handle, Error> {
        Handle::new(session, session.send(open(path, flags)))
    }

    fn new(session: &Arc<Session>, opened: Pending) -> Result<Handle, Error> {
        Ok(Handle {
            id: opened.wait()?.handle()?,
            session: session.clone(),
        })
    }

    pub(crate) fn open_stat(session: &Arc<Session>, path: &str) -> Result<(Handle, Attrs), Error> {
        let opened = session.send(open(path, FXF_READ));
        let stat = session.send(Packet::new(STAT).str(path));
        let handle = Handle::new(session, opened)?;
        Ok((handle, stat.wait()?.attrs()?))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.session.send(Packet::new(CLOSE).bytes(&self.id));
    }
}

fn open(path: &str, flags: u32) -> Packet {
    Packet::new(OPEN)
        .str(path)
        .u32(flags)
        .attrs(&Attrs::default())
}

#[derive(Clone)]
pub struct Server {
    pub(crate) inner: Arc<Inner>,
}

impl Server {
    /// Logs in through `/usr/bin/ssh`; with no `askpass` helper ssh runs in batch mode.
    pub fn connect(address: &Address, askpass: Option<&Path>) -> Result<Server, Error> {
        let spawn = Spawn::Ssh {
            address: address.clone(),
            askpass: askpass.map(Askpass::new).transpose()?,
        };
        Server::start(address.clone(), spawn)
    }

    /// Speaks SFTP to whatever `make` starts, such as `sftp-server` itself.
    #[doc(hidden)]
    pub fn connect_command(
        make: impl Fn() -> Command + Send + Sync + 'static,
    ) -> Result<Server, Error> {
        let address = Address {
            user: None,
            host: String::new(),
            port: None,
            path: String::new(),
            identity: None,
        };
        Server::start(address, Spawn::Command(Box::new(make)))
    }

    fn start(address: Address, spawn: Spawn) -> Result<Server, Error> {
        let (session, version) = spawn.session()?;
        let extensions = session.hello(version)?;
        let limits = extensions
            .iter()
            .any(|name| name == "limits@openssh.com")
            .then(|| session.send(Packet::new(EXTENDED).str("limits@openssh.com")));
        let home = session.send(Packet::new(REALPATH).str("."));
        let (mut read_size, mut write_size) = (CHUNK, CHUNK);
        if let Some(Ok(reply)) = limits.map(Pending::wait) {
            let mut limits = reply.extended()?;
            let [_packet, read, write] = [limits.u64()?, limits.u64()?, limits.u64()?];
            let cap = |limit: u64| match limit {
                0 => CHUNK,
                limit => limit.min(CHUNK.into()) as u32,
            };
            (read_size, write_size) = (cap(read), cap(write));
        }
        let home = home.wait()?.name()?;
        let mut sessions: [Option<Arc<Session>>; 1 + POOL] = Default::default();
        sessions[0] = Some(session);
        Ok(Server {
            inner: Arc::new(Inner {
                address,
                spawn,
                home,
                extensions,
                read_size,
                write_size,
                pool: Mutex::new(Pool {
                    closed: false,
                    sessions,
                }),
            }),
        })
    }

    pub fn address(&self) -> &Address {
        &self.inner.address
    }

    pub fn home(&self) -> &str {
        &self.inner.home
    }

    pub fn extensions(&self) -> &[String] {
        &self.inner.extensions
    }

    fn has(&self, extension: &str) -> bool {
        self.inner.extensions.iter().any(|name| name == extension)
    }

    fn call(&self, packet: Packet) -> Result<Reply, Error> {
        self.inner.session(0)?.call(packet)
    }

    /// The directory's entries without `.` and `..`.
    pub fn list(&self, dir: &str) -> Result<Vec<Entry>, Error> {
        let session = self.inner.session(0)?;
        let handle = Handle::new(&session, session.send(Packet::new(OPENDIR).str(dir)))?;
        let read = || session.send(Packet::new(READDIR).bytes(&handle.id));
        let mut pending: VecDeque<Pending> = (0..READ_AHEAD).map(|_| read()).collect();
        let mut entries = Vec::new();
        while let Some(next) = pending.pop_front() {
            match next.wait().and_then(|reply| reply.names()) {
                Ok(names) => {
                    let names = names.into_iter();
                    entries.extend(names.filter(|entry| entry.name != "." && entry.name != ".."));
                    pending.push_back(read());
                }
                Err(error) if error.is_eof() => break,
                Err(error) => return Err(error),
            }
        }
        Ok(entries)
    }

    /// Does not follow a final symlink.
    pub fn stat(&self, path: &str) -> Result<Attrs, Error> {
        self.call(Packet::new(LSTAT).str(path))?.attrs()
    }

    pub fn stat_follow(&self, path: &str) -> Result<Attrs, Error> {
        self.call(Packet::new(STAT).str(path))?.attrs()
    }

    pub fn readlink(&self, path: &str) -> Result<String, Error> {
        self.call(Packet::new(READLINK).str(path))?.name()
    }

    /// OpenSSH takes the target first, the reverse of the draft it implements.
    pub fn symlink(&self, target: &str, link: &str) -> Result<(), Error> {
        self.call(Packet::new(SYMLINK).str(target).str(link))?.ok()
    }

    pub fn realpath(&self, path: &str) -> Result<String, Error> {
        self.call(Packet::new(REALPATH).str(path))?.name()
    }

    pub fn mkdir(&self, path: &str) -> Result<(), Error> {
        self.call(Packet::new(MKDIR).str(path).attrs(&Attrs::default()))?
            .ok()
    }

    pub fn rmdir(&self, path: &str) -> Result<(), Error> {
        self.call(Packet::new(RMDIR).str(path))?.ok()
    }

    pub fn remove(&self, path: &str) -> Result<(), Error> {
        self.call(Packet::new(REMOVE).str(path))?.ok()
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
        let packet = if self.has("posix-rename@openssh.com") {
            Packet::new(EXTENDED).str("posix-rename@openssh.com")
        } else {
            Packet::new(RENAME)
        };
        self.call(packet.str(from).str(to))?.ok()
    }

    pub fn set_mtime(&self, path: &str, mtime: u32) -> Result<(), Error> {
        let attrs = Attrs {
            mtime: Some(mtime),
            ..Attrs::default()
        };
        self.call(Packet::new(SETSTAT).str(path).attrs(&attrs))?
            .ok()
    }

    pub fn set_permissions(&self, path: &str, mode: u32) -> Result<(), Error> {
        let attrs = Attrs {
            permissions: Some(mode),
            ..Attrs::default()
        };
        self.call(Packet::new(SETSTAT).str(path).attrs(&attrs))?
            .ok()
    }

    /// None when the server cannot say.
    pub fn statvfs(&self, path: &str) -> Option<Space> {
        if !self.has("statvfs@openssh.com") {
            return None;
        }
        let packet = Packet::new(EXTENDED).str("statvfs@openssh.com").str(path);
        let reply = self.call(packet).ok()?;
        let mut stats = reply.extended().ok()?;
        let mut next = || stats.u64().ok();
        let [_bsize, frsize, blocks, _bfree, available] =
            [next()?, next()?, next()?, next()?, next()?];
        Some(Space {
            available: available * frsize,
            total: blocks * frsize,
        })
    }

    /// Copies a whole file on the server; None when the server cannot.
    pub fn copy_data(&self, from: &str, to: &str) -> Option<Result<(), Error>> {
        self.has("copy-data").then(|| {
            let session = self.inner.session(1)?;
            let source = Handle::open(&session, from, FXF_READ)?;
            let target = Handle::open(&session, to, FXF_WRITE | FXF_CREAT | FXF_TRUNC)?;
            let packet = Packet::new(EXTENDED)
                .str("copy-data")
                .bytes(&source.id)
                .u64(0)
                .u64(0)
                .bytes(&target.id)
                .u64(0);
            session.call(packet)?.ok()
        })
    }

    /// Kills every connection; later calls return [`Error::Disconnected`].
    pub fn disconnect(&self) {
        let mut pool = self.inner.pool.lock();
        pool.closed = true;
        for session in pool.sessions.iter_mut().filter_map(Option::take) {
            session.kill();
        }
        if let Spawn::Ssh {
            askpass: Some(askpass),
            ..
        } = &self.inner.spawn
        {
            let _ = fs::remove_dir_all(&askpass.dir);
        }
    }
}
