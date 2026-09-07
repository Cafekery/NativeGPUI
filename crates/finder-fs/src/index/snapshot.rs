//! The on-disk index: the header, the name blob and the two arrays, read back with one `read`
//! each. Little-endian and native layout, so it is a cache for this machine, not a format.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::store::{NONE, Names, Node, Root, Tree};

const MAGIC: &[u8; 8] = b"NGVIDX03";
const HEADER: usize = 88;

pub fn epoch(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn field(head: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(head[8 + i * 8..16 + i * 8].try_into().unwrap())
}

/// # Safety
/// `T` must be an integer or a `repr(C)` struct of integers: every bit pattern is a value.
unsafe fn read_vec<T>(file: &mut File, n: usize) -> std::io::Result<Vec<T>> {
    let mut out: Vec<T> = Vec::with_capacity(n);
    let bytes = std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, n * size_of::<T>());
    file.read_exact(bytes)?;
    out.set_len(n);
    Ok(out)
}

fn as_bytes<T>(values: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, size_of_val(values)) }
}

pub fn load(path: &Path) -> Option<Tree> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; HEADER];
    file.read_exact(&mut head).ok()?;
    if &head[..8] != MAGIC {
        return None;
    }
    let (nodes, names, blob) = (field(&head, 0), field(&head, 1), field(&head, 2));
    let roots = field(&head, 8);
    let expected =
        HEADER as u64 + roots + names * 4 + blob + nodes * 4 + nodes * size_of::<Node>() as u64;
    if nodes == 0 || file.metadata().ok()?.len() != expected {
        return None;
    }
    let mut root_bytes = vec![0u8; roots as usize];
    file.read_exact(&mut root_bytes).ok()?;
    let roots = read_roots(&root_bytes).filter(|roots| roots.len() as u64 == field(&head, 9))?;
    let ends = unsafe { read_vec(&mut file, names as usize) }.ok()?;
    let mut bytes = vec![0u8; blob as usize];
    file.read_exact(&mut bytes).ok()?;
    let name_of = unsafe { read_vec(&mut file, nodes as usize) }.ok()?;
    let nodes = unsafe { read_vec(&mut file, nodes as usize) }.ok()?;
    let mut tree = Tree {
        nodes,
        name_of,
        names: Names::with_blob(bytes, ends),
        dirs: field(&head, 5),
        files: field(&head, 6),
        bytes: field(&head, 7),
        crawled_at: (field(&head, 3) != 0)
            .then(|| UNIX_EPOCH + Duration::from_secs(field(&head, 3))),
        event_id: field(&head, 4),
        roots,
    };
    if !sound(&tree) {
        return None;
    }
    tree.names.reindex();
    Some(tree)
}

/// The roots section: an `event_id`, an mtime and a length-prefixed path each, back to back.
fn read_roots(mut bytes: &[u8]) -> Option<Vec<Root>> {
    let mut roots = Vec::new();
    while !bytes.is_empty() {
        let head: &[u8; 16] = bytes.get(..16)?.try_into().ok()?;
        let len = u32::from_le_bytes(head[12..16].try_into().ok()?) as usize;
        let path = bytes.get(16..16 + len)?;
        roots.push(Root {
            path: PathBuf::from(OsStr::from_bytes(path)),
            event_id: u64::from_le_bytes(head[..8].try_into().ok()?),
            modified: u32::from_le_bytes(head[8..12].try_into().ok()?),
        });
        bytes = &bytes[16 + len..];
    }
    Some(roots)
}

fn write_roots(roots: &[Root]) -> Vec<u8> {
    let mut out = Vec::new();
    for root in roots {
        let path = root.path.as_os_str().as_bytes();
        out.extend_from_slice(&root.event_id.to_le_bytes());
        out.extend_from_slice(&root.modified.to_le_bytes());
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path);
    }
    out
}

fn sound(tree: &Tree) -> bool {
    let (nodes, names) = (tree.nodes.len() as u32, tree.names.len() as u32);
    let ends = tree.names.ends.last().copied().unwrap_or(0) as usize;
    if ends != tree.names.blob.len() {
        return false;
    }
    let within = |id: u32| id == NONE || id < nodes;
    // A node's parent is an earlier node, which is what ends a walk to the root.
    tree.name_of.iter().all(|&id| id < names)
        && tree
            .nodes
            .iter()
            .enumerate()
            .all(|(id, n)| id == 0 || n.parent < id as u32)
        && tree
            .nodes
            .iter()
            .all(|n| within(n.first_child) && within(n.next_sibling))
}

pub fn save(tree: &Tree, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    let mut file = File::create(&temp)?;
    let roots = write_roots(&tree.roots);
    let mut head = Vec::with_capacity(HEADER);
    head.extend_from_slice(MAGIC);
    for value in [
        tree.nodes.len() as u64,
        tree.names.len() as u64,
        tree.names.blob.len() as u64,
        tree.crawled_at.map_or(0, epoch),
        tree.event_id,
        tree.dirs,
        tree.files,
        tree.bytes,
        roots.len() as u64,
        tree.roots.len() as u64,
    ] {
        head.extend_from_slice(&value.to_le_bytes());
    }
    file.write_all(&head)?;
    file.write_all(&roots)?;
    file.write_all(as_bytes(&tree.names.ends))?;
    file.write_all(&tree.names.blob)?;
    file.write_all(as_bytes(&tree.name_of))?;
    file.write_all(as_bytes(&tree.nodes))?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
}
