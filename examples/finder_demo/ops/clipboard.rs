//! Finder's clipboard: ⌘C writes the names and the file URLs, ⌘X marks the selection (rows
//! dim until pasted or the mark clears), ⌘V moves a cut selection or copies a copied one.

use std::path::PathBuf;

use finder_fs::ops::Op;
use gpui::{Context, SharedString, Window};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::app::FinderDemo;
use crate::data::split_key;
use crate::menus::CopyNames;
use crate::ops::After;

impl FinderDemo {
    pub(crate) fn copy_names(&mut self, _: &CopyNames, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    /// Finder's ⌘C: the names as text and the files as file URLs, so a paste in Finder lands them.
    /// A copy also clears any pending ⌘X mark, Finder's own convention.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.clear_cut(cx);
        let keys: Vec<SharedString> = self
            .focused_lines()
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.key.clone())
            .collect();
        if keys.is_empty() {
            return;
        }
        let names: Vec<&str> = keys
            .iter()
            .filter_map(|key| split_key(key).map(|(_, name)| name))
            .collect();
        let text = NSString::from_str(&names.join("\n"));
        let mut items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
            vec![ProtocolObject::from_retained(text)];
        items.extend(keys.iter().map(|key| {
            ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(key)))
        }));
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.writeObjects(&NSArray::from_retained_slice(&items));
    }

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
