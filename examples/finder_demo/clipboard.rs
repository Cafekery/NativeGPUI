//! Finder's ⌘X/⌘V: Cut marks the selection (rows dim until pasted or the mark clears), Paste
//! moves a cut selection or copies a ⌥⌘V/⌘C one. ⌘C still writes file URLs, unchanged.

use std::path::PathBuf;

use finder_fs::ops::Op;
use gpui::{Context, SharedString};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::FinderDemo;
use crate::ops::After;

impl FinderDemo {
    /// ⌘X: the selection copies to the pasteboard and is marked cut until pasted or cleared.
    pub(crate) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<SharedString> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.key.clone())
            .collect();
        if keys.is_empty() {
            return;
        }
        write_pasteboard(&keys);
        self.cut = keys.into_iter().collect();
        cx.notify();
    }

    /// Esc, or another Copy/Cut: Finder clears the cut mark without touching disk.
    pub(crate) fn clear_cut(&mut self, cx: &mut Context<Self>) {
        if self.cut.is_empty() {
            return;
        }
        self.cut.clear();
        cx.notify();
    }

    /// ⌘V: moves a cut selection into the folder on screen; a plain ⌘C selection copies there.
    /// ⌥⌘V ("Move Item Here") always moves, even from a plain copy.
    pub(crate) fn paste(&mut self, force_move: bool, cx: &mut Context<Self>) {
        let items = read_pasteboard();
        if items.is_empty() {
            return;
        }
        let into = PathBuf::from(self.target());
        let op = if force_move || !self.cut.is_empty() {
            Op::Move { items, into }
        } else {
            Op::Copy { items, into }
        };
        self.cut.clear();
        self.run_op(op, After::Select, cx);
    }
}

fn write_pasteboard(keys: &[SharedString]) {
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = keys
        .iter()
        .map(|key| ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(key))))
        .collect();
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    board.writeObjects(&NSArray::from_retained_slice(&items));
}

fn read_pasteboard() -> Vec<PathBuf> {
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

/// Whether the general pasteboard holds files a Paste row could act on, cheap enough to call
/// while a context menu is being built.
pub(crate) fn can_paste() -> bool {
    let file_url = unsafe { NSPasteboardTypeFileURL };
    NSPasteboard::generalPasteboard()
        .pasteboardItems()
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.stringForType(file_url).is_some())
        })
}
