//! What a menu pick does: every contextual row and every File-menu command lands here.

use std::path::PathBuf;

use finder_fs::ops::{Conflict, Op};
use gpui::{Context, Window};

use super::context;
use crate::app::FinderDemo;
use crate::data::Column;
use crate::open_finder_window;
use crate::ops::After;
use crate::views::header::ColumnSpec;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Act {
    Nothing,
    Swatch,
    NewFolderWith,
    Groups,
    ViewOptions,
    GetInfo,
    GetInfoAt(&'static str),
    NewFolder,
    Inspector,
    InspectorAt(&'static str),
    Sort(Column),
    Open,
    OpenWindow,
    Trash,
    Rename,
    Duplicate,
    Copy,
    Cut,
    Paste,
    NewFile,
    OpenTerminal(&'static str),
    OpenTab(&'static str),
    OpenWindowAt(&'static str),
    Enclosing(&'static str),
    CopyPathname(&'static str),
    Toggle(Column),
}

/// Open in Terminal: the user's `TERM_PROGRAM` if a GUI launch carries one, else Terminal.app.
fn open_terminal(path: &str) {
    let app = std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "Terminal".to_owned());
    let _ = std::process::Command::new("open")
        .args(["-a", &app])
        .arg(path)
        .spawn();
}

impl FinderDemo {
    pub(crate) fn run_menu(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Nothing => return,
            Act::Swatch => match context::picked_swatch() {
                Some(tag) => self.tag_selection(tag, cx),
                None => return,
            },
            Act::NewFolder => self.new_folder(cx),
            Act::NewFolderWith => self.new_folder_with_selection(cx),
            Act::Groups => self.grouped = !self.grouped,
            Act::ViewOptions => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::options::open(&demo, cx));
            }
            Act::Inspector => {
                self.preview = true;
                self.inspecting = None;
            }
            Act::InspectorAt(id) => {
                self.preview = true;
                self.inspecting = crate::data::path_line(id);
            }
            Act::GetInfo => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, None, cx));
            }
            Act::GetInfoAt(id) => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, Some(id), cx));
            }
            Act::Sort(column) => self.sort_by(column, cx),
            Act::Open => self.open_here(window, cx),
            Act::OpenWindow => self.open_in_window(window, cx),
            Act::Trash => self.trash_selection(cx),
            Act::Rename => self.renaming = self.cursor.clone(),
            Act::Duplicate => self.duplicate_selection(cx),
            Act::Copy => self.copy_selection(cx),
            Act::Cut => self.cut_selection(cx),
            Act::Paste => self.paste(false, cx),
            Act::NewFile => self.new_file(cx),
            Act::OpenTerminal(path) => open_terminal(path),
            Act::OpenTab(id) => self.open_tab(id, window, cx),
            Act::OpenWindowAt(id) => {
                open_finder_window(id, 1, cx);
            }
            Act::Enclosing(id) => self.show_enclosing(id, window, cx),
            Act::CopyPathname(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string()))
            }
            Act::Toggle(column) => self.toggle_column(column),
        }
        cx.notify();
    }

    pub(crate) fn new_folder(&mut self, cx: &mut Context<Self>) {
        self.make_folder("untitled folder", Vec::new(), cx);
    }

    /// Finder's ⌃⌘N: the chosen rows move into a folder made for them.
    pub(crate) fn new_folder_with_selection(&mut self, cx: &mut Context<Self>) {
        let chosen = self.paths();
        if chosen.is_empty() {
            return;
        }
        self.make_folder("New Folder With Items", chosen, cx);
    }

    pub(crate) fn new_file(&mut self, cx: &mut Context<Self>) {
        let op = Op::NewFile {
            into: PathBuf::from(self.target()),
            name: "untitled.txt".to_owned(),
        };
        self.run_op(op, After::Rename, cx);
    }

    fn make_folder(&mut self, stem: &str, with: Vec<PathBuf>, cx: &mut Context<Self>) {
        let op = Op::NewFolder {
            into: PathBuf::from(self.target()),
            name: stem.to_owned(),
        };
        if with.is_empty() {
            self.run_op(op, After::Rename, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move { finder_fs::ops::perform(op, Conflict::KeepBoth, |_| {}) })
                .await;
            let Some(folder) = made.ok().and_then(|done| done.results.first().cloned()) else {
                return;
            };
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.run_op(
                    Op::Move {
                        items: with,
                        into: folder,
                    },
                    After::Nothing,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.chosen_keys()
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect()
    }

    fn duplicate_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if !items.is_empty() {
            self.run_op(Op::Duplicate(items), After::Select, cx);
        }
    }

    pub(crate) fn trash_selection(&mut self, cx: &mut Context<Self>) {
        self.remove_selection(Op::Trash, cx);
    }

    pub(crate) fn delete_selection(&mut self, cx: &mut Context<Self>) {
        self.remove_selection(Op::DeleteImmediately, cx);
    }

    /// The chosen rows leave the listing with the operation that takes them off it.
    fn remove_selection(&mut self, op: fn(Vec<PathBuf>) -> Op, cx: &mut Context<Self>) {
        let items = self.paths();
        if items.is_empty() {
            return;
        }
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.run_op(op(items), After::Nothing, cx);
    }

    fn open_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let opens = self.chosen_folders();
        if let Some(first) = opens.first() {
            self.go(first, window, cx);
        }
    }

    /// A second window, handed to AppKit as a tab of this one.
    pub(crate) fn open_tab(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) {
        let host = window.window_handle();
        cx.defer(move |cx| {
            let tab = open_finder_window(id, 1, cx);
            tab.update(cx, |_, tab, cx| native_gpui::add_tab(host, tab, cx))
                .ok();
        });
    }

    fn open_in_window(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        for (extra, id) in self.chosen_folders().into_iter().enumerate() {
            open_finder_window(id, extra + 1, cx);
        }
    }

    pub(crate) fn sort_by(&mut self, column: Column, cx: &mut Context<Self>) {
        let ascending = match self.sort == column {
            true => !self.ascending,
            false => true,
        };
        self.resort((column, ascending), cx);
    }

    fn toggle_column(&mut self, column: Column) {
        match self.list_columns.iter().position(|s| s.column == column) {
            Some(index) => {
                self.list_columns.remove(index);
            }
            None => self.list_columns.push(ColumnSpec::of(column)),
        }
        self.refit_name_column();
    }
}
