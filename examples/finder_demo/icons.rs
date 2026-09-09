//! The artwork a file type wears: drawn once per type and size, on the thread that wants it
//! first — the one reading the folder, wherever it can.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, Entity, px, size};
use native_gpui::tokens::{FileType, Symbol, file_icon};

use crate::app::FinderDemo;
use crate::data::Row;

/// A type and the size it is drawn at.
type Key = (FileType<'static>, u32);

/// How long a batch of rows may spend drawing its types before the rest wait for a frame to ask.
const WARM_BUDGET: Duration = Duration::from_millis(20);

/// Every icon drawn: its artwork, or nothing where the type has none. Shared, because the thread
/// that reads a folder draws its types before the main thread ever sees the rows.
static ICONS: LazyLock<Mutex<HashMap<Key, Option<Symbol>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

thread_local! {
    static WANTED: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

/// The system's artwork for a type at this size, or the nearest size already drawn.
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

/// The types every window needs whatever it lists: a row falls back to one of these while its
/// own type is still being drawn, so it never falls back to an SF glyph.
pub(crate) const STAND_INS: [FileType<'static>; 5] = [
    FileType::Folder,
    FileType::FullFolder,
    FileType::Path("/"),
    FileType::Extension("app"),
    FileType::Uti("public.data"),
];

/// The types a batch of rows wears, drawn on the thread that read them, so the frame that first
/// shows the rows already has their artwork rather than a stand-in. A folder of many types cannot
/// hold its own listing up: past the budget the rest wait for a frame to ask.
pub(crate) fn warm(rows: &[Arc<Row>], size: f32) {
    let until = Instant::now() + WARM_BUDGET;
    for kind in undrawn(rows.iter().filter_map(|row| row.artwork), size) {
        if Instant::now() >= until {
            return;
        }
        draw(kind, size);
    }
}

/// The stand-ins, drawn on the calling thread: a window draws them before its first frame.
pub(crate) fn draw_stand_ins(size: f32) {
    for kind in undrawn(STAND_INS, size) {
        draw(kind, size);
    }
}

/// Which of these types have no artwork at this size yet, each once.
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
    let drawn = file_icon(kind, px(size));
    ICONS.lock().unwrap().insert((kind, size.to_bits()), drawn);
}

/// What is drawn for the type already, worn at the size asked for: a zoom asks a size a frame.
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

/// Draw what the frames since the last call asked for; every window that draws a row calls it.
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
                    ICONS.lock().unwrap().insert(key, drawn);
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
