use std::path::{Path, PathBuf};

use finder_fs::ops::{Op, Restore};

const ORIGINS: &str = "put-back.tsv";

fn read() -> Vec<(PathBuf, PathBuf)> {
    let text = std::fs::read_to_string(crate::store().join(ORIGINS)).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let (item, to) = line.split_once('\t')?;
            Some((PathBuf::from(item), PathBuf::from(to)))
        })
        .collect()
}

fn restores(op: &Op, into: &mut Vec<Restore>) {
    match op {
        Op::PutBack(items) => into.extend(items.iter().cloned()),
        Op::Batch(ops) => ops.iter().for_each(|op| restores(op, into)),
        _ => {}
    }
}

/// `NSFileManager` leaves no put-back record in the Trash, so Found'er keeps its own.
pub(crate) fn remember(undo: &Op) {
    let mut trashed = Vec::new();
    restores(undo, &mut trashed);
    if trashed.is_empty() {
        return;
    }
    let fits = |path: &Path| {
        path.to_str()
            .is_some_and(|text| !text.contains(['\t', '\n']))
    };
    let fresh = (trashed.into_iter())
        .filter(|restore| fits(&restore.item) && fits(&restore.to))
        .map(|restore| (restore.item, restore.to));
    let mut kept = read();
    kept.extend(fresh);
    let mut text = String::new();
    for (item, to) in kept
        .iter()
        .filter(|(item, _)| item.symlink_metadata().is_ok())
    {
        text.push_str(&format!("{}\t{}\n", item.display(), to.display()));
    }
    std::fs::write(crate::store().join(ORIGINS), text).ok();
}

pub(crate) fn origins(items: &[PathBuf]) -> Vec<Option<PathBuf>> {
    let finder = finder_fs::put_back::origins(items);
    if finder.iter().all(Option::is_some) {
        return finder;
    }
    let ours = read();
    (finder.into_iter().zip(items))
        .map(|(origin, item)| {
            origin.or_else(|| {
                (ours.iter().rev())
                    .find(|(trashed, _)| trashed == item)
                    .map(|(_, to)| to.clone())
            })
        })
        .collect()
}
