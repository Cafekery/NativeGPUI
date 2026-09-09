use std::collections::{HashMap, HashSet};

use gpui::SharedString;

use super::card::STACK_LAYERS;
use crate::data::{Face, Line, Row, day_string};

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
    pub(crate) tags: Vec<usize>,
    pub(crate) head: Vec<Line>,
}

/// Runs off the main thread, so nothing here may touch a view.
pub(crate) fn summarize(
    key: (u64, usize),
    lines: &[Line],
    chosen: &HashSet<SharedString>,
    tags: &HashMap<SharedString, Vec<usize>>,
) -> Stats {
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
    let picked = lines
        .iter()
        .filter(|line| chosen.contains(&line.key))
        .take(STATS_CAP);
    for line in picked {
        let row = &line.row;
        stats.items += 1;
        stats.folders += usize::from(row.face == Face::Folder);
        stats.bytes += total_bytes(row);
        stats.flat_bytes += row.bytes;
        stats.created = (
            stats.created.0.min(row.created_at),
            stats.created.1.max(row.created_at),
        );
        stats.modified = (
            stats.modified.0.min(row.modified_at),
            stats.modified.1.max(row.modified_at),
        );
        if stats.head.len() < STACK_LAYERS.len() {
            stats.head.push(line.clone());
        }
        for tag in tags.get(&line.key).map(Vec::as_slice).unwrap_or_default() {
            if !stats.tags.contains(tag) {
                stats.tags.push(*tag);
            }
        }
    }
    stats.tags.sort_unstable();
    stats
}

fn total_bytes(row: &Row) -> u64 {
    match row.opens.filter(|_| row.discloses) {
        Some(id) => crate::index()
            .folder_size(std::path::Path::new(id))
            .unwrap_or(row.bytes),
        None => row.bytes,
    }
}

pub(super) fn span((lo, hi): (f64, f64)) -> SharedString {
    let (lo, hi) = (day_string(lo), day_string(hi));
    SharedString::from(if lo == hi {
        lo
    } else {
        format!("{lo} \u{2013} {hi}")
    })
}
