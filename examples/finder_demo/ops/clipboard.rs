use std::cell::Cell;
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

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.clear_cut(cx);
        let keys = self.chosen_keys();
        if keys.is_empty() {
            return;
        }
        let names: Vec<&str> = keys
            .iter()
            .filter_map(|key| split_key(key).map(|(_, name)| name))
            .collect();
        let text = NSString::from_str(&names.join("\n"));
        let mut items: Vec<Writable> = vec![ProtocolObject::from_retained(text)];
        items.extend(file_urls(&keys));
        write_pasteboard(&items);
    }

    pub(crate) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        let keys = self.chosen_keys();
        if keys.is_empty() {
            return;
        }
        write_pasteboard(&file_urls(&keys).collect::<Vec<_>>());
        self.cut = keys.into_iter().collect();
        cx.notify();
    }

    pub(crate) fn clear_cut(&mut self, cx: &mut Context<Self>) {
        if self.cut.is_empty() {
            return;
        }
        self.cut.clear();
        cx.notify();
    }

    pub(crate) fn paste(&mut self, force_move: bool, cx: &mut Context<Self>) {
        let items = read_pasteboard();
        if items.is_empty() {
            return;
        }
        let into = PathBuf::from(self.target());
        // Files copied since, in any app, may have replaced the cut on the pasteboard.
        let cut = !self.cut.is_empty()
            && items.iter().all(|item| {
                let key = SharedString::from(item.to_string_lossy().into_owned());
                self.cut.contains(&key)
            });
        let op = if force_move || cut {
            Op::Move { items, into }
        } else {
            Op::Copy { items, into }
        };
        self.cut.clear();
        self.run_op(op, After::Select, cx);
    }
}

type Writable = Retained<ProtocolObject<dyn NSPasteboardWriting>>;

fn file_urls(keys: &[SharedString]) -> impl Iterator<Item = Writable> + '_ {
    keys.iter()
        .map(|key| ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(key))))
}

fn write_pasteboard(items: &[Writable]) {
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    board.writeObjects(&NSArray::from_retained_slice(items));
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

pub(crate) fn can_paste() -> bool {
    thread_local! {
        static ANSWERED: Cell<(isize, bool)> = const { Cell::new((-1, false)) };
    }
    let board = NSPasteboard::generalPasteboard();
    let changed = board.changeCount();
    if let Some(answer) = ANSWERED.with(|seen| {
        let (at, answer) = seen.get();
        (at == changed).then_some(answer)
    }) {
        return answer;
    }
    // Types only: fetching the data waits on whichever app promised it, for as long as it takes.
    let file_url = unsafe { NSPasteboardTypeFileURL };
    let answer = board.canReadItemWithDataConformingToTypes(&NSArray::from_slice(&[file_url]));
    ANSWERED.with(|seen| seen.set((changed, answer)));
    answer
}
