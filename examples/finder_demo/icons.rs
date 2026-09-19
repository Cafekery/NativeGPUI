use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, Entity, px, size};
use native_gpui::tokens::{FileType, Symbol, file_icon};

use crate::app::FinderDemo;
use crate::data::Row;

type Key = (FileType<'static>, u32);

const WARM_BUDGET: Duration = Duration::from_millis(20);

const ICONS_KEPT: usize = 2048;

/// The icon service answers four requests in little more time than one, and no more past four.
const LANES: usize = 4;

/// How far past the rows on screen a listing draws ahead.
const LOOK_AHEAD: usize = 512;

static ICONS: LazyLock<Mutex<HashMap<Key, Option<Symbol>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The rest of a listing, newest last, drawn only while no row on screen is waiting.
static AHEAD: LazyLock<Mutex<Vec<Key>>> = LazyLock::new(|| Mutex::new(Vec::new()));

thread_local! {
    static WANTED: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
    /// Asked for and not drawn, newest last, so a scroll draws what it landed on, not what it left.
    static PENDING: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
    static DRAWING: Cell<usize> = const { Cell::new(0) };
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
    let mut ahead = Vec::new();
    for kind in undrawn(rows.iter().filter_map(|row| row.artwork), size, LOOK_AHEAD) {
        if Instant::now() < until {
            draw(kind, size);
        } else {
            ahead.push((kind, size.to_bits()));
        }
    }
    if ahead.is_empty() {
        return;
    }
    // Reversed, so that taking from the end draws the listing in row order.
    ahead.reverse();
    let mut queue = AHEAD.lock().unwrap();
    queue.extend(ahead);
    let over = queue.len().saturating_sub(LOOK_AHEAD);
    queue.drain(..over);
}

pub(crate) fn draw_stand_ins(size: f32) {
    for kind in undrawn(STAND_INS, size, STAND_INS.len()) {
        draw(kind, size);
    }
}

fn undrawn(
    kinds: impl IntoIterator<Item = FileType<'static>>,
    size: f32,
    most: usize,
) -> Vec<FileType<'static>> {
    let icons = ICONS.lock().unwrap();
    let mut seen = HashSet::new();
    kinds
        .into_iter()
        .filter(|kind| !icons.contains_key(&(*kind, size.to_bits())) && seen.insert(*kind))
        .take(most)
        .collect()
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

/// A row that is waiting, or else the listing drawn ahead of one; the flag is which.
fn next_to_draw() -> Option<(Key, bool)> {
    if let Some(key) = PENDING.with(|pending| pending.borrow_mut().pop()) {
        return Some((key, true));
    }
    loop {
        let key = AHEAD.lock().unwrap().pop()?;
        if !ICONS.lock().unwrap().contains_key(&key) {
            return Some((key, false));
        }
    }
}

pub(crate) fn draw_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted = WANTED.with(|wanted| wanted.take());
    if !wanted.is_empty() {
        for key in &wanted {
            let waiting = nearest(key.0, f32::from_bits(key.1));
            ICONS.lock().unwrap().entry(*key).or_insert(waiting);
        }
        // Reversed, so that taking from the end draws a frame's rows top down.
        PENDING.with(|pending| pending.borrow_mut().extend(wanted.into_iter().rev()));
    }
    while DRAWING.with(Cell::get) < LANES {
        let Some(first) = next_to_draw() else {
            return;
        };
        DRAWING.with(|drawing| drawing.set(drawing.get() + 1));
        let demo = demo.downgrade();
        cx.spawn(async move |cx| {
            let mut next = Some(first);
            while let Some((key, waited_for)) = next {
                let drawn = cx
                    .background_executor()
                    .spawn(async move { file_icon(key.0, px(f32::from_bits(key.1))) })
                    .await;
                let held = demo.update(cx, |_, cx| {
                    keep(key, drawn);
                    if waited_for {
                        cx.notify();
                    }
                });
                if held.is_err() {
                    break;
                }
                next = next_to_draw();
            }
            DRAWING.with(|drawing| drawing.set(drawing.get() - 1));
        })
        .detach();
    }
}
