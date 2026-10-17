use std::collections::HashMap;
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use parking_lot::Mutex;

use crate::Error;
use crate::wire::{Cursor, INIT, Packet, Reply, VERSION};

/// A longer length is text a login shell printed into the channel, not SFTP.
const MAX_PACKET: usize = 4 << 20;

#[derive(Default)]
struct Waiters {
    dead: bool,
    map: HashMap<u32, SyncSender<Reply>>,
}

pub(crate) struct Session {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    waiters: Arc<Mutex<Waiters>>,
    next_id: AtomicU32,
    stderr: Mutex<Option<JoinHandle<String>>>,
    attempt: Option<PathBuf>,
}

pub(crate) struct Pending(Receiver<Reply>);

impl Pending {
    pub(crate) fn wait(self) -> Result<Reply, Error> {
        self.0.recv().map_err(|_| Error::Disconnected)
    }
}

impl Session {
    pub(crate) fn spawn(
        mut command: Command,
        attempt: Option<PathBuf>,
    ) -> Result<(Arc<Session>, Pending), Error> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .inspect_err(|_| {
                if let Some(attempt) = &attempt {
                    let _ = std::fs::remove_file(attempt);
                }
            })?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let stderr = thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr.read_to_end(&mut text);
            String::from_utf8_lossy(&text).trim().to_owned()
        });
        let (version, reply) = sync_channel(1);
        let waiters = Arc::new(Mutex::new(Waiters::default()));
        waiters.lock().map.insert(0, version);
        thread::spawn({
            let waiters = waiters.clone();
            move || read(BufReader::with_capacity(1 << 18, stdout), &waiters)
        });
        let session = Session {
            stdin: Mutex::new(child.stdin.take().unwrap()),
            child: Mutex::new(child),
            waiters,
            next_id: AtomicU32::new(1),
            stderr: Mutex::new(Some(stderr)),
            attempt,
        };
        // INIT carries the protocol version where every other request carries its id.
        let _ = session.write(Packet::new(INIT), 3);
        Ok((Arc::new(session), Pending(reply)))
    }

    pub(crate) fn hello(&self, version: Pending) -> Result<Vec<String>, Error> {
        let Ok(reply) = version.wait() else {
            return Err(Error::Connect(self.stderr()));
        };
        extensions(&reply.body).inspect_err(|_| self.kill())
    }

    fn stderr(&self) -> String {
        self.kill();
        let _ = self.child.lock().wait();
        let text = self
            .stderr
            .lock()
            .take()
            .and_then(|reader| reader.join().ok());
        text.filter(|text| !text.is_empty())
            .unwrap_or_else(|| "the connection closed before SFTP started".into())
    }

    pub(crate) fn send(&self, packet: Packet) -> Pending {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (waiter, reply) = sync_channel(1);
        {
            let mut waiters = self.waiters.lock();
            if waiters.dead {
                return Pending(reply);
            }
            waiters.map.insert(id, waiter);
        }
        if self.write(packet, id).is_err() {
            self.waiters.lock().map.remove(&id);
        }
        Pending(reply)
    }

    pub(crate) fn call(&self, packet: Packet) -> Result<Reply, Error> {
        self.send(packet).wait()
    }

    fn write(&self, mut packet: Packet, id: u32) -> io::Result<()> {
        let len = packet.0.len() as u32 - 4;
        packet.0[..4].copy_from_slice(&len.to_be_bytes());
        packet.0[5..9].copy_from_slice(&id.to_be_bytes());
        self.stdin.lock().write_all(&packet.0)
    }

    pub(crate) fn alive(&self) -> bool {
        !self.waiters.lock().dead
    }

    pub(crate) fn kill(&self) {
        let _ = self.child.lock().kill();
        fail(&self.waiters);
        if let Some(attempt) = &self.attempt {
            let _ = std::fs::remove_file(attempt);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
        let _ = self.child.lock().wait();
    }
}

fn extensions(version: &[u8]) -> Result<Vec<String>, Error> {
    let mut cursor = Cursor(version);
    let version = cursor.u32()?;
    if version < 3 {
        return Err(Error::Protocol(format!(
            "the server speaks SFTP version {version}"
        )));
    }
    let mut extensions = Vec::new();
    while !cursor.0.is_empty() {
        extensions.push(cursor.string()?);
        cursor.bytes()?;
    }
    Ok(extensions)
}

fn fail(waiters: &Mutex<Waiters>) {
    let mut waiters = waiters.lock();
    waiters.dead = true;
    waiters.map.clear();
}

fn read(mut stdout: impl Read, waiters: &Mutex<Waiters>) {
    while let Ok((id, reply)) = frame(&mut stdout) {
        let waiter = waiters.lock().map.remove(&id);
        if let Some(waiter) = waiter {
            let _ = waiter.send(reply);
        }
    }
    fail(waiters);
}

fn frame(stdout: &mut impl Read) -> io::Result<(u32, Reply)> {
    let mut len = [0; 4];
    stdout.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if !(1..=MAX_PACKET).contains(&len) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut kind = [0];
    stdout.read_exact(&mut kind)?;
    let [kind] = kind;
    let header = if kind == VERSION { 1 } else { 5 };
    let rest = len.checked_sub(header).ok_or(io::ErrorKind::InvalidData)?;
    let mut id = [0; 4];
    if kind != VERSION {
        stdout.read_exact(&mut id)?;
    }
    let mut body = vec![0; rest];
    stdout.read_exact(&mut body)?;
    Ok((u32::from_be_bytes(id), Reply { kind, body }))
}
