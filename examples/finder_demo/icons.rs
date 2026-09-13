use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, Entity, px, size};
use native_gpui::tokens::{FileType, Symbol, file_icon};

use crate::app::FinderDemo;
use crate::data::Row;

type Key = (FileType<'static>, u32);

const WARM_BUDGET: Duration = Duration::from_millis(20);

const ICONS_KEPT: usize = 2048;

static ICONS: LazyLock<Mutex<HashMap<Key, Option<Symbol>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    static WANTED: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn icon(kind: FileType<'static>, size: f32) -> Option<Symbol> {
    let key = (kind, size.to_bits());
    if let Some(icon) = ICONS.lock().unwrap().get(&key) {
        return icon.clone();
    }
    WANTED.with(|wanted| {
        let mut wanted = wanted.borrow_mut();
        if !wanted.contains(&key) {
            wanted.push(key);
        }
    });
    nearest(kind, size)
}

pub(crate) const STAND_INS: [FileType<'static>; 5] = [
    FileType::Folder,
    FileType::FullFolder,
    FileType::Path("/"),
    FileType::Extension("app"),
    FileType::Uti("public.data"),
];

pub(crate) fn warm(rows: &[Arc<Row>], size: f32) {
    let until = Instant::now() + WARM_BUDGET;
    for kind in undrawn(rows.iter().filter_map(|row| row.artwork), size) {
        if Instant::now() >= until {
            return;
        }
        draw(kind, size);
    }
}

pub(crate) fn draw_stand_ins(size: f32) {
    for kind in undrawn(STAND_INS, size) {
        draw(kind, size);
    }
}

fn undrawn(
    kinds: impl IntoIterator<Item = FileType<'static>>,
    size: f32,
) -> Vec<FileType<'static>> {
    let icons = ICONS.lock().unwrap();
    let mut undrawn = Vec::new();
    for kind in kinds {
        if !icons.contains_key(&(kind, size.to_bits())) && !undrawn.contains(&kind) {
            undrawn.push(kind);
        }
    }
    undrawn
}

fn draw(kind: FileType<'static>, size: f32) {
    keep((kind, size.to_bits()), file_icon(kind, px(size)));
}

fn keep(key: Key, drawn: Option<Symbol>) {
    let mut icons = ICONS.lock().unwrap();
    if icons.len() >= ICONS_KEPT {
        icons.retain(|(kind, _), _| !matches!(kind, FileType::Path(_) | FileType::FullPath(_)));
    }
    icons.insert(key, drawn);
}

fn nearest(kind: FileType<'static>, at: f32) -> Option<Symbol> {
    ICONS
        .lock()
        .unwrap()
        .iter()
        .filter_map(|((drawn, _), icon)| (*drawn == kind).then_some(icon.as_ref()?))
        .min_by(|a, b| {
            (f32::from(a.size.width) - at)
                .abs()
                .total_cmp(&(f32::from(b.size.width) - at).abs())
        })
        .map(|icon| Symbol {
            size: size(px(at), px(at)),
            ..icon.clone()
        })
}

pub(crate) fn draw_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted = WANTED.with(|wanted| wanted.take());
    if wanted.is_empty() {
        return;
    }
    for key in &wanted {
        let waiting = nearest(key.0, f32::from_bits(key.1));
        ICONS.lock().unwrap().entry(*key).or_insert(waiting);
    }
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        for key in wanted {
            let drawn = cx
                .background_executor()
                .spawn(async move { file_icon(key.0, px(f32::from_bits(key.1))) })
                .await;
            if demo
                .update(cx, |_, cx| {
                    keep(key, drawn);
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
        }
    })
    .detach();
}
