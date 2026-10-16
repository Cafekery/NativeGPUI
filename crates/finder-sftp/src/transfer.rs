use std::collections::VecDeque;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::server::{Handle, POOL};
use crate::session::Session;
use crate::wire::*;
use crate::{Attrs, Error, Server};

const SPLIT_AT: u64 = 16 << 20;
const WINDOW: usize = 64;
const TICK: Duration = Duration::from_millis(50);

/// Byte range shared by every session working on one file: each takes the next chunk.
struct Job {
    next: AtomicU64,
    end: u64,
    done: AtomicU64,
    cancel: AtomicBool,
    gate: Mutex<()>,
}

impl Job {
    fn new(end: u64) -> Job {
        Job {
            next: AtomicU64::new(0),
            end,
            done: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            gate: Mutex::new(()),
        }
    }

    fn claim(&self, chunk: u32) -> Option<(u64, u32)> {
        drop(self.gate.lock());
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let offset = self.next.fetch_add(chunk.into(), Ordering::Relaxed);
        (offset < self.end).then(|| (offset, (self.end - offset).min(chunk.into()) as u32))
    }

    fn checked(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Server {
    /// Writes the remote file to `local`; `progress(done, total)` returning false cancels.
    pub fn download(
        &self,
        remote: &str,
        local: &Path,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), Error> {
        let session = self.inner.session(1)?;
        let (handle, attrs) = Handle::open_stat(&session, remote)?;
        let size = attrs.size.unwrap_or(0);
        let file = File::create(local)?;
        let result = (|| {
            file.set_len(size)?;
            let job = Job::new(if size == 0 { u64::MAX } else { size });
            let sink = |offset: u64, data: &[u8]| file.write_all_at(data, offset);
            self.transfer(
                handle,
                &job,
                size,
                &|session| Handle::open(session, remote, FXF_READ),
                &|handle| fetch(handle, &job, self.inner.read_size, &sink),
                progress,
            )?;
            if let Some(mtime) = attrs.mtime {
                file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime.into()))?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(local);
        }
        result
    }

    /// Writes `local` to the remote path with its mode and modification time.
    pub fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), Error> {
        let file = File::open(local)?;
        let meta = file.metadata()?;
        let session = self.inner.session(1)?;
        let handle = Handle::open(&session, remote, FXF_WRITE | FXF_CREAT | FXF_TRUNC)?;
        let job = Job::new(meta.len());
        let result = self.transfer(
            handle,
            &job,
            meta.len(),
            &|session| Handle::open(session, remote, FXF_WRITE),
            &|handle| push(handle, &job, self.inner.write_size, &file),
            progress,
        );
        if result.is_err() {
            let _ = session.call(Packet::new(REMOVE).str(remote));
            return result;
        }
        let attrs = Attrs {
            permissions: Some(meta.mode() & 0o777),
            atime: Some(meta.atime() as u32),
            mtime: Some(meta.mtime() as u32),
            ..Attrs::default()
        };
        let _ = session.call(Packet::new(SETSTAT).str(remote).attrs(&attrs));
        Ok(())
    }

    /// The whole file, for previews; [`Error::Protocol`] when it is larger than `limit`.
    pub fn read_all(&self, remote: &str, limit: u64) -> Result<Vec<u8>, Error> {
        let session = self.inner.session(1)?;
        let (handle, attrs) = Handle::open_stat(&session, remote)?;
        let too_large = || Error::Protocol(format!("{remote} is larger than {limit} bytes"));
        let size = attrs.size.unwrap_or(0);
        if size > limit {
            return Err(too_large());
        }
        let bytes = Mutex::new(Vec::new());
        let job = Job::new(if size == 0 { limit + 1 } else { size });
        let sink = |offset: u64, data: &[u8]| {
            let mut bytes = bytes.lock();
            let (start, end) = (offset as usize, offset as usize + data.len());
            if bytes.len() < end {
                bytes.resize(end, 0);
            }
            bytes[start..end].copy_from_slice(data);
            Ok(())
        };
        fetch(&handle, &job, self.inner.read_size, &sink)?;
        let bytes = bytes.into_inner();
        if bytes.len() as u64 > limit {
            return Err(too_large());
        }
        Ok(bytes)
    }

    /// Runs `work` on `first` and, for a large file, on further pool sessions as they come up,
    /// reporting progress from this thread.
    fn transfer(
        &self,
        first: Handle,
        job: &Job,
        total: u64,
        open: &(dyn Fn(&Arc<Session>) -> Result<Handle, Error> + Sync),
        work: &(dyn Fn(&Handle) -> Result<(), Error> + Sync),
        progress: &mut dyn FnMut(u64, u64) -> bool,
    ) -> Result<(), Error> {
        let (finished, results) = channel();
        thread::scope(|scope| {
            scope.spawn({
                let finished = finished.clone();
                move || finished.send(work(&first))
            });
            let workers = if total >= SPLIT_AT { POOL } else { 1 };
            for slot in 2..=workers {
                let finished = finished.clone();
                scope.spawn(move || {
                    let Ok(session) = self.inner.session(slot) else {
                        return;
                    };
                    let _ = finished.send(open(&session).and_then(|handle| work(&handle)));
                });
            }
            drop(finished);
            let mut result = Ok(());
            let mut report = |result: &mut Result<(), Error>| {
                let _gate = job.gate.lock();
                let done = job.done.load(Ordering::Relaxed);
                if result.is_ok() && !progress(done, total.max(done)) {
                    *result = Err(Error::Cancelled);
                    job.cancel.store(true, Ordering::Relaxed);
                }
            };
            report(&mut result);
            loop {
                match results.recv_timeout(TICK) {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        result = result.and(Err(error));
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                    Err(RecvTimeoutError::Timeout) => report(&mut result),
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            report(&mut result);
            result
        })
    }
}

/// Reads chunks with `WINDOW` requests outstanding until the job runs out or the file ends.
fn fetch(
    handle: &Handle,
    job: &Job,
    chunk: u32,
    sink: &(dyn Fn(u64, &[u8]) -> io::Result<()> + Sync),
) -> Result<(), Error> {
    let read = |offset: u64, len: u32| {
        let packet = Packet::new(READ).bytes(&handle.id).u64(offset).u32(len);
        (offset, len, handle.session.send(packet))
    };
    let mut queue = VecDeque::new();
    let mut more = true;
    loop {
        while more && queue.len() < WINDOW {
            match job.claim(chunk) {
                Some((offset, len)) => queue.push_back(read(offset, len)),
                None => more = false,
            }
        }
        let Some((offset, len, pending)) = queue.pop_front() else {
            return job.checked();
        };
        let got = pending.wait().and_then(|reply| {
            let data = reply.data()?;
            sink(offset, data)?;
            Ok(data.len() as u32)
        });
        match got {
            Ok(0) => more = false,
            Ok(got) => {
                job.done.fetch_add(got.into(), Ordering::Relaxed);
                if got < len {
                    queue.push_back(read(offset + u64::from(got), len - got));
                }
            }
            Err(error) if error.is_eof() => more = false,
            Err(error) => return Err(error),
        }
        job.checked()?;
    }
}

/// Writes chunks of `file` with `WINDOW` requests outstanding until the job runs out.
fn push(handle: &Handle, job: &Job, chunk: u32, file: &File) -> Result<(), Error> {
    let mut queue = VecDeque::new();
    let mut more = true;
    loop {
        while more && queue.len() < WINDOW {
            let Some((offset, len)) = job.claim(chunk) else {
                more = false;
                break;
            };
            let mut packet = Packet::new(WRITE).bytes(&handle.id).u64(offset).u32(len);
            let start = packet.0.len();
            packet.0.resize(start + len as usize, 0);
            file.read_exact_at(&mut packet.0[start..], offset)?;
            queue.push_back((len, handle.session.send(packet)));
        }
        let Some((len, pending)) = queue.pop_front() else {
            return job.checked();
        };
        pending.wait()?.ok()?;
        job.done.fetch_add(len.into(), Ordering::Relaxed);
        job.checked()?;
    }
}
