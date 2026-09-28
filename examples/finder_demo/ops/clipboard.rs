use std::cell::Cell;
use std::path::PathBuf;

use finder_fs::ops::Op;
use gpui::{Context, Window};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::app::FinderDemo;
use crate::data::split_key;
use crate::menus::CopyNames;
use crate::ops::After;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Pasting {
    Copy,
    Move,
    Exactly,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Held {
    pub(crate) files: usize,
    pub(crate) movable: bool,
}

const FINDER_NODES: &str = "com.apple.finder.noderef";

thread_local! {
    static OWN_CHANGE: Cell<isize> = const { Cell::new(-1) };
    static HELD: Cell<(isize, Held)> = const {
        Cell::new((
            -1,
            Held {
                files: 0,
                movable: false,
            },
        ))
    };
}

impl FinderDemo {
    pub(crate) fn copy_names(&mut self, _: &CopyNames, _: &mut Window, _: &mut Context<Self>) {
        self.copy_selection();
    }

    pub(crate) fn copy_selection(&self) {
        let keys = self.chosen_keys();
        if keys.is_empty() {
            return;
        }
        let names: Vec<&str> = keys
            .iter()
            .filter_map(|key| split_key(key).map(|(_, name)| name))
            .collect();
        let text = NSString::from_str(&names.join("\n"));
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            std::iter::once(ProtocolObject::from_retained(text))
                .chain(keys.iter().map(|key| {
                    ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(key)))
                }))
                .collect();
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.writeObjects(&NSArray::from_retained_slice(&items));
        OWN_CHANGE.set(board.changeCount());
    }

    pub(crate) fn copy_pathnames(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let text = match paths.is_empty() {
            true => self.target().to_owned(),
            false => paths.join("\n"),
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    pub(crate) fn paste(&mut self, how: Pasting, cx: &mut Context<Self>) {
        self.paste_into(PathBuf::from(self.target()), how, cx);
    }

    pub(crate) fn paste_into(&mut self, into: PathBuf, how: Pasting, cx: &mut Context<Self>) {
        if !into.is_absolute() {
            return;
        }
        let items = read_pasteboard();
        if items.is_empty() {
            return;
        }
        let op = match how {
            Pasting::Exactly => Op::CopyExactly { items, into },
            Pasting::Move => Op::Move { items, into },
            Pasting::Copy => Op::Copy { items, into },
        };
        self.run_op(op, After::Select, cx);
    }
}

pub(crate) fn read_pasteboard() -> Vec<PathBuf> {
    let board = NSPasteboard::generalPasteboard();
    let Some(items) = board.pasteboardItems() else {
        return Vec::new();
    };
    let file_url = unsafe { NSPasteboardTypeFileURL };
    items
        .iter()
        .filter_map(|item| item.stringForType(file_url))
        .filter_map(|url| NSURL::URLWithString(&url))
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

pub(crate) fn held() -> Held {
    let board = NSPasteboard::generalPasteboard();
    let changed = board.changeCount();
    let (at, known) = HELD.get();
    if at == changed {
        return known;
    }
    // Types only: fetching the data waits on whichever app promised it, for as long as it takes.
    let file_url = unsafe { NSPasteboardTypeFileURL };
    let finder = NSString::from_str(FINDER_NODES);
    let (mut files, mut from_finder) = (0, false);
    for item in board
        .pasteboardItems()
        .iter()
        .flat_map(|items| items.iter())
    {
        let types = item.types();
        if types.containsObject(file_url) {
            files += 1;
            from_finder |= types.containsObject(&finder);
        }
    }
    let held = Held {
        files,
        movable: files > 0 && (from_finder || OWN_CHANGE.get() == changed),
    };
    HELD.set((changed, held));
    held
}

pub(crate) fn can_paste() -> bool {
    held().files > 0
}
