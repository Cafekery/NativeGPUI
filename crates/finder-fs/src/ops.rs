//! File operations with Finder's semantics: copies and moves through NSFileManager (packages,
//! metadata and permissions intact), Trash with Put Back, and an undo journal.
//!
//! Contract (stub; the real implementation keeps these signatures): `perform` runs on the caller's
//! thread — callers spawn; progress is reported per item; conflicts follow `Conflict`; every
//! `Done` carries the operation that undoes it when one exists.

use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Op {
    Move { items: Vec<PathBuf>, into: PathBuf },
    Copy { items: Vec<PathBuf>, into: PathBuf },
    Trash(Vec<PathBuf>),
    DeleteImmediately(Vec<PathBuf>),
    Rename { path: PathBuf, name: String },
    Duplicate(Vec<PathBuf>),
    NewFolder { into: PathBuf, name: String },
    NewFile { into: PathBuf, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// `name 2`, `name 3`, … like Duplicate.
    KeepBoth,
    Replace,
    /// Folders merge, files inside follow `Replace`.
    Merge,
    Skip,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Done {
    /// Where each item ended up, in input order.
    pub results: Vec<PathBuf>,
    pub undo: Option<Op>,
}

pub fn perform(_op: Op, _conflict: Conflict, _progress: impl FnMut(Progress)) -> io::Result<Done> {
    Err(io::Error::other("not implemented"))
}
