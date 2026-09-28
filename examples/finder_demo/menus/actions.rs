use std::path::{Path, PathBuf};

use finder_fs::ops::{Conflict, Op, Restore};
use gpui::{Bounds, Context, SharedString, Window, point, px, size};
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{ShareItem, SharePicker, SurfaceAnchor, present_share_picker};
use objc2_foundation::{NSString, NSURL, NSURLBookmarkResolutionOptions};

use super::context;
use crate::app::FinderDemo;
use crate::data::{Column, is_trash};
use crate::ops::After;
use crate::{open_finder_tab, open_finder_window};

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
    DeleteNow,
    EmptyTrash,
    Rename,
    Duplicate,
    MakeAlias,
    QuickLook,
    OpenWith(&'static str),
    Compress,
    Share,
    Tags,
    Copy,
    Cut,
    Paste,
    NewFile,
    OpenTerminal(&'static str),
    OpenTab(&'static str),
    OpenWindowAt(&'static str),
    Enclosing(&'static str),
    RemoveFromSidebar(&'static str),
    RenameAt(&'static str),
    Eject(&'static str),
    CopyPathname(&'static str),
    CopyPathnames,
    Slideshow,
    MoveHere,
    Toggle(Column),
}

fn original_of(path: &Path) -> Option<PathBuf> {
    if let Ok(target) = std::fs::read_link(path) {
        let joined = match target.is_absolute() {
            true => target,
            false => path.parent()?.join(target),
        };
        return joined.canonicalize().ok();
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let resolved = NSURL::URLByResolvingAliasFileAtURL_options_error(
        &url,
        NSURLBookmarkResolutionOptions::WithoutUI | NSURLBookmarkResolutionOptions::WithoutMounting,
    )
    .ok()?;
    let original = PathBuf::from(resolved.path()?.to_string());
    // Resolving a plain file hands back the file itself.
    (original != path.canonicalize().ok()?).then_some(original)
}

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
                self.show_preview(true);
                self.inspecting = None;
            }
            Act::InspectorAt(id) => {
                self.show_preview(true);
                self.inspecting = crate::data::path_line(id);
            }
            Act::GetInfo => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, None, false, cx));
            }
            Act::GetInfoAt(id) => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::info::open(&demo, Some(id), false, cx));
            }
            Act::Sort(column) => self.sort_by(column, cx),
            Act::Open => self.open_selected(window, cx),
            Act::OpenWindow => self.open_in_window(cx),
            Act::Trash => self.trash_selection(cx),
            Act::DeleteNow => self.delete_selection(cx),
            Act::EmptyTrash => self.empty_trash(cx),
            Act::Rename => self.renaming = self.cursor.clone(),
            Act::Duplicate => self.with_selection(Op::Duplicate, cx),
            Act::MakeAlias => self.with_selection(Op::MakeAlias, cx),
            Act::Compress => self.with_selection(Op::Compress, cx),
            Act::Share => self.share_selection(window, cx),
            Act::Tags => self.edit_tags(window, cx),
            Act::QuickLook => self.toggle_quick_look(window, cx),
            Act::OpenWith(app) => crate::openwith::open_with(&self.paths(), Path::new(app)),
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
            Act::RemoveFromSidebar(id) => self.remove_from_sidebar(id),
            Act::RenameAt(id) => {
                self.show_enclosing(id, window, cx);
                self.renaming = Some(id.into());
            }
            Act::Eject(id) => self.eject(id, window, cx),
            Act::CopyPathname(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string()))
            }
            Act::CopyPathnames => {
                let lines: Vec<String> = self
                    .paths()
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(lines.join("\n")))
            }
            Act::Slideshow => self.slideshow(window, cx),
            Act::MoveHere => self.paste(true, cx),
            Act::Toggle(column) => self.toggle_column(column, cx),
        }
        cx.notify();
    }

    pub(crate) fn new_folder(&mut self, cx: &mut Context<Self>) {
        self.make_folder("untitled folder", Vec::new(), cx);
    }

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

    fn with_selection(&mut self, op: fn(Vec<PathBuf>) -> Op, cx: &mut Context<Self>) {
        let items = self.paths();
        if !items.is_empty() {
            self.run_op(op(items), After::Select, cx);
        }
    }

    fn share_urls(&self) -> Vec<ShareItem> {
        self.paths()
            .iter()
            .filter_map(|path| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                url.absoluteString()
                    .map(|url| ShareItem::Url(url.to_string()))
            })
            .collect()
    }

    pub(crate) fn airdrop_selection(&mut self) {
        let items = self.share_urls();
        if !items.is_empty() && !native_gpui::airdrop(&items) {
            self.complain("AirDrop isn’t available for the selected items.");
        }
    }

    pub(crate) fn share_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.share_urls();
        if items.is_empty() {
            return;
        }
        let at = match self.ql_icon.bounds() {
            Some(_) => self.ql_icon.clone(),
            None => {
                let width = window.viewport_size().width;
                SurfaceAnchor::at(Bounds::new(
                    point(width / 2., px(60.)),
                    size(px(1.), px(1.)),
                ))
            }
        };
        let picker = SharePicker {
            edge: SurfaceEdge::Below,
            ..Default::default()
        };
        present_share_picker(window, cx, &at, picker, items, |_, _, _| {});
    }

    pub(crate) fn show_original(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self.paths().first().and_then(|path| original_of(path)) else {
            return;
        };
        self.show_enclosing(crate::data::intern_path(&original), window, cx);
    }

    pub(crate) fn eject_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let roots = crate::data::volume_roots();
        let mut wanted = self.chosen_folders();
        wanted.push(self.here());
        let Some(volume) = wanted.into_iter().find(|id| roots.contains(id)) else {
            return;
        };
        self.eject(volume, window, cx);
    }

    pub(crate) fn empty_trash(&mut self, cx: &mut Context<Self>) {
        let Some(trash) = finder_fs::volumes::standard(finder_fs::volumes::Standard::Trash) else {
            return;
        };
        let items: Vec<PathBuf> = finder_fs::list::list_dir(&trash)
            .unwrap_or_default()
            .iter()
            .map(|entry| entry.path_in(&trash))
            .collect();
        if items.is_empty() {
            return;
        }
        let this = cx.entity().downgrade();
        self.confirm(
            "Are you sure you want to permanently erase the items in the Trash?",
            "You can’t undo this action.",
            "Empty Trash",
            false,
            cx,
            move |cx| {
                let mut cx = cx.clone();
                crate::sound::Sound::EmptyTrash.play();
                this.update(&mut cx, |this: &mut FinderDemo, cx| {
                    this.run_op(Op::DeleteImmediately(items.clone()), After::Nothing, cx);
                })
                .ok();
            },
        );
    }

    pub(crate) fn trash_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if items.is_empty() {
            return;
        }
        if !is_trash(self.here()) {
            self.remove(Op::Trash(items), cx);
            return;
        }
        let back: Vec<Restore> = finder_fs::put_back::origins(&items)
            .into_iter()
            .zip(items)
            .filter_map(|(to, item)| Some(Restore { item, to: to? }))
            .collect();
        if !back.is_empty() {
            self.deselect();
            self.run_op(Op::PutBack(back), After::Reveal, cx);
        }
    }

    pub(crate) fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        let (text, detail) = match items.as_slice() {
            [] => return,
            [only] => (
                format!(
                    "Are you sure you want to delete \u{201c}{}\u{201d}?",
                    only.file_name().unwrap_or_default().to_string_lossy()
                ),
                "This item will be deleted immediately. You can\u{2019}t undo this action.".into(),
            ),
            _ => (
                format!(
                    "Are you sure you want to delete the {} selected items?",
                    items.len()
                ),
                format!(
                    "{} items will be deleted immediately. You can\u{2019}t undo this action.",
                    items.len()
                ),
            ),
        };
        let this = cx.entity().downgrade();
        self.confirm(&text, &detail, "Delete", true, cx, move |cx| {
            let mut cx = cx.clone();
            this.update(&mut cx, |this: &mut FinderDemo, cx| {
                this.remove(Op::DeleteImmediately(items.clone()), cx);
            })
            .ok();
        });
    }

    fn remove(&mut self, op: Op, cx: &mut Context<Self>) {
        let kept = self.kept_row_below_cursor();
        self.choose(kept);
        self.run_op(op, After::Nothing, cx);
    }

    fn kept_row_below_cursor(&self) -> Option<SharedString> {
        let lines = self.focused_lines();
        let next = &lines
            .get(self.line_at(&lines, self.cursor.as_ref()?)? + 1)?
            .key;
        let goes = |gone: &SharedString| {
            next.strip_prefix(&**gone)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        (!self.selected.iter().any(goes)).then(|| next.clone())
    }

    pub(crate) fn open_tab(&self, id: &'static str, window: &Window, cx: &mut Context<Self>) {
        let host = window.window_handle();
        let bounds = window.window_bounds();
        let start = match id.starts_with("tag:") {
            true => self.here(),
            false => id,
        };
        cx.defer(move |cx| {
            let tab = open_finder_tab(start, bounds, cx);
            tab.update(cx, |demo: &mut FinderDemo, tab, cx| {
                if start != id {
                    demo.show_tag(id, tab, cx);
                }
                native_gpui::add_tab(host, tab, cx);
            })
            .ok();
        });
    }

    fn open_in_window(&self, cx: &mut Context<Self>) {
        for (extra, id) in self.chosen_folders().into_iter().enumerate() {
            open_finder_window(id, extra + 1, cx);
        }
    }
}
