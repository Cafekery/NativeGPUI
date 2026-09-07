//! The artwork a file type wears: drawn off the main thread, once per type and size.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{App, Entity, px, size};
use native_gpui::tokens::{FileType, Symbol, file_icon};

use crate::app::FinderDemo;

/// A type and the size it is drawn at.
type Key = (FileType<'static>, u32);

thread_local! {
    /// Every icon asked for: its artwork, or nothing where the type has none.
    static ICONS: RefCell<HashMap<Key, Option<Symbol>>> = RefCell::new(HashMap::new());
    static WANTED: RefCell<Vec<Key>> = const { RefCell::new(Vec::new()) };
}

/// The system's artwork for a type at this size, or the nearest size already drawn.
pub(crate) fn icon(kind: FileType<'static>, size: f32) -> Option<Symbol> {
    let key = (kind, size.to_bits());
    if let Some(icon) = ICONS.with(|icons| icons.borrow().get(&key).cloned()) {
        return icon;
    }
    WANTED.with(|wanted| {
        let mut wanted = wanted.borrow_mut();
        if !wanted.contains(&key) {
            wanted.push(key);
        }
    });
    nearest(kind, size)
}

/// What is drawn for the type already, worn at the size asked for: a zoom asks a size a frame.
fn nearest(kind: FileType<'static>, at: f32) -> Option<Symbol> {
    ICONS.with(|icons| {
        icons
            .borrow()
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
        ICONS.with(|icons| icons.borrow_mut().insert(*key, waiting));
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
                    ICONS.with(|icons| icons.borrow_mut().insert(key, drawn));
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
