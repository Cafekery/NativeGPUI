use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::SharedString;

use super::card::STACK_LAYERS;
use crate::data::{Face, Line, Row, day_string};
use crate::tags::{RowTag, all, row_tags};

pub(super) const CALCULATING: &str = "Calculating\u{2026}";

const STATS_CAP: usize = 4_000_000;

pub(crate) struct Stats {
    pub(crate) key: (u64, usize),
    pub(crate) items: usize,
    pub(crate) folders: usize,
    pub(crate) bytes: u64,
    pub(crate) flat_bytes: u64,
    pub(crate) created: (f64, f64),
    pub(crate) modified: (f64, f64),
    pub(crate) tags: Vec<RowTag>,
    pub(crate) head: Vec<Line>,
}

/// The items a range or click just added, newest first, for the stack's top layers.
#[derive(Clone, Default)]
pub(crate) struct Newest {
    pub(crate) cursor: Option<SharedString>,
    pub(crate) anchor: Option<SharedString>,
}

pub(crate) fn summarize<'a>(
    key: (u64, usize),
    picked: impl IntoIterator<Item = &'a Line>,
    newest: &Newest,
    stale: &AtomicBool,
) -> Option<Stats> {
    let mut stats = Stats {
        key,
        items: 0,
        folders: 0,
        bytes: 0,
        flat_bytes: 0,
        created: (f64::MAX, f64::MIN),
        modified: (f64::MAX, f64::MIN),
        tags: Vec::new(),
        head: Vec::new(),
    };
    let mut weighed = HashMap::new();
    let layers = STACK_LAYERS.len();
    let (mut first, mut before, mut after) = (Vec::new(), Vec::<&Line>::new(), Vec::new());
    let (mut top, mut anchor_above) = (None, false);
    let widen = |(lo, hi): (f64, f64), at: f64| (lo.min(at), hi.max(at));
    for line in picked.into_iter().take(STATS_CAP) {
        if stale.load(Ordering::Relaxed) {
            return None;
        }
        let row = &line.row;
        stats.items += 1;
        stats.folders += usize::from(row.face == Face::Folder);
        stats.bytes += total_bytes(row, &mut weighed, stale)?;
        stats.flat_bytes += row.bytes;
        stats.created = widen(stats.created, row.created_at);
        stats.modified = widen(stats.modified, row.modified_at);
        if top.is_some() {
            if after.len() < layers - 1 {
                after.push(line);
            }
        } else if newest.cursor.as_ref() == Some(&line.key) {
            top = Some(line);
        } else {
            anchor_above |= newest.anchor.as_ref() == Some(&line.key);
            if before.len() == layers - 1 {
                before.remove(0);
            }
            before.push(line);
        }
        if first.len() < layers {
            first.push(line);
        }
        for tag in row_tags(&line.key) {
            if !stats.tags.iter().any(|held| held.name == tag.name) {
                stats.tags.push(tag);
            }
        }
    }
    let head: Vec<&Line> = match top {
        Some(top) => {
            let nearest_before = before.into_iter().rev();
            let rest: Vec<&Line> = match anchor_above || after.is_empty() {
                true => nearest_before.chain(after).collect(),
                false => after.into_iter().chain(nearest_before).collect(),
            };
            std::iter::once(top).chain(rest).take(layers).collect()
        }
        None => first,
    };
    stats.head = head.into_iter().cloned().collect();
    let listed = all();
    stats.tags.sort_by_key(|tag| {
        listed
            .iter()
            .position(|it| *it.name == *tag.name)
            .unwrap_or(usize::MAX)
    });
    Some(stats)
}

type Weighed = HashMap<&'static Path, HashMap<String, (Option<u64>, Option<usize>)>>;

pub(crate) fn sized_by_index(row: &Row) -> Option<&'static str> {
    row.opens.filter(|_| row.discloses)
}

fn total_bytes(row: &Row, weighed: &mut Weighed, stale: &AtomicBool) -> Option<u64> {
    if crate::remote::is_remote(&row.path) {
        return row.opens.is_none().then_some(row.bytes);
    }
    let Some(id) = sized_by_index(row) else {
        return Some(row.bytes);
    };
    let path = Path::new(id);
    let size = match (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
    ) {
        (Some(dir), Some(name)) => weighed
            .entry(dir)
            .or_insert_with(|| crate::fsearch::folders_in(dir))
            .get(name)
            .and_then(|(size, _)| *size),
        _ => crate::fsearch::totals(path).map(|totals| totals.bytes),
    };
    match size {
        Some(size) => Some(size),
        None if row.face == Face::Volume => Some(row.bytes),
        None => walk(path, stale),
    }
}

fn walk(folder: &Path, stale: &AtomicBool) -> Option<u64> {
    let mut bytes = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if stale.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) => bytes += meta.len(),
                Err(_) => {}
            }
        }
    }
    Some(bytes)
}

pub(super) fn span((lo, hi): (f64, f64)) -> SharedString {
    let (lo, hi) = (day_string(lo), day_string(hi));
    SharedString::from(if lo == hi {
        lo
    } else {
        format!("{lo} \u{2013} {hi}")
    })
}
