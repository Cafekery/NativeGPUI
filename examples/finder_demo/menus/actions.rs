use std::path::{Path, PathBuf};

use finder_fs::ops::{Conflict, Op, Restore};
use gpui::{
    App, Bounds, Context, Pixels, Point, SharedString, Window, WindowHandle, point, px, size,
};
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{ShareItem, SharePicker, SurfaceAnchor, present_share_picker};
use objc2_foundation::{NSString, NSURL, NSURLBookmarkResolutionOptions};

use super::context;
use crate::app::FinderDemo;
use crate::data::{Column, intern_path, is_trash};
use crate::ops::After;
use crate::ops::clipboard::Pasting;
use crate::prefs::{self, WindowTarget};
use crate::search::RECENTS;
use crate::sound::Sound;
use crate::views::icon::Arrangement;
use crate::{home, open_finder_tab, open_finder_window};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Act {
    Nothing,
    Swatch,
    NewFolderWith,
    Groups,
    GroupBy(Option<Column>),
    ViewOptions,
    View(usize),
    GetInfo,
    GetInfoAt(&'static str),
    SummaryInfo,
    NewFolder,
    Inspector,
    InspectorAt(&'static str),
    Sort(Column),
    Arrange(Arrangement),
    CleanUp,
    CleanUpBy(Column),
    Open,
    OpenTabs,
    OpenWindow,
    ShowOriginal,
    Trash,
    DeleteNow,
    EmptyTrash,
    Rename,
    Duplicate,
    MakeAlias,
    QuickLook,
    OpenWith(&'static str),
    AlwaysOpenWith(&'static str),
    Compress,
    Share,
    PreviewOptions,
    Copy,
    Paste(&'static str, Pasting),
    OpenTerminal(&'static str),
    OpenTab(&'static str),
    OpenWindowAt(&'static str),
    AddToDock(&'static str),
    Enclosing(&'static str),
    RemoveFromSidebar(&'static str),
    TagColor(&'static str, u8),
    RenameAt(&'static str),
    Eject(&'static str),
    CopyPathname(&'static str),
    CopyPathnames,
    Slideshow,
    Toggle(Column),
    Quick(crate::quick_actions::QuickAct),
}

pub(super) fn original_of(path: &Path) -> Option<PathBuf> {
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

pub(crate) fn new_window_target() -> &'static str {
    match prefs::new_window_target() {
        WindowTarget::Recents => RECENTS,
        WindowTarget::Computer => crate::locations::COMPUTER,
        WindowTarget::Folder(path) => intern_path(&path),
    }
}

fn trash_items() -> Vec<PathBuf> {
    let Some(trash) = finder_fs::volumes::standard(finder_fs::volumes::Standard::Trash) else {
        return Vec::new();
    };
    finder_fs::list::list_dir(&trash)
        .unwrap_or_default()
        .iter()
        .map(|entry| entry.path_in(&trash))
        .collect()
}

impl FinderDemo {
    pub(crate) fn run_menu(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        match act {
            Act::Nothing => return,
            Act::Swatch => match context::picked_swatch() {
                Some(Some(tag)) => self.tag_selection(tag, cx),
                Some(None) => self.tags_or_customize(window, cx),
                None => return,
            },
            Act::NewFolder => self.new_folder(cx),
            Act::NewFolderWith => self.new_folder_with_selection(cx),
            Act::Groups => self.toggle_groups(cx),
            Act::GroupBy(group) => self.set_group(group, cx),
            Act::ViewOptions => {
                let demo = cx.entity();
                cx.defer(move |cx| crate::options::open(&demo, cx));
            }
            Act::View(view) => self.pick_view(view, window, cx),
            Act::Inspector => crate::inspector::toggle(self, cx),
            Act::InspectorAt(id) => crate::inspector::show_item(id, self, cx),
            Act::GetInfo | Act::GetInfoAt(_) | Act::SummaryInfo => {
                let demo = cx.entity();
                let at = match act {
                    Act::GetInfoAt(id) => Some(id),
                    _ => None,
                };
                let summary = act == Act::SummaryInfo;
                cx.defer(move |cx| crate::info::open(&demo, at, summary, cx));
            }
            Act::Sort(column) => self.sort_by(column, cx),
            Act::Arrange(arrangement) => self.arrange(arrangement, cx),
            Act::CleanUp => self.clean_up(false, cx),
            Act::CleanUpBy(column) => self.clean_up_by(column, window, cx),
            Act::Open => self.open_selected(window, cx),
            Act::OpenTabs => {
                for id in self.chosen_folders() {
                    self.open_tab(id, window, cx);
                }
            }
            Act::OpenWindow => open_windows(self.chosen_folders(), cx),
            Act::ShowOriginal => self.show_original(window, cx),
            Act::Trash => self.trash_selection(cx),
            Act::DeleteNow => self.delete_selection(cx),
            Act::EmptyTrash => self.empty_trash(cx),
            Act::Rename => self.renaming = self.cursor.clone(),
            Act::Duplicate => self.with_selection(Op::Duplicate, After::Select, cx),
            Act::MakeAlias => self.with_selection(Op::MakeAlias, After::Rename, cx),
            Act::Compress => self.with_selection(Op::Compress, After::Select, cx),
            Act::Share => self.share_selection(window, cx),
            Act::PreviewOptions => crate::preview::options::toggle(self, cx),
            Act::QuickLook => self.toggle_quick_look(window, cx),
            Act::OpenWith(app) => crate::openwith::open_with(&self.paths(), Path::new(app)),
            Act::AlwaysOpenWith(app) => {
                let paths = self.paths();
                crate::openwith::set_default(&paths, Path::new(app));
                crate::openwith::open_with(&paths, Path::new(app));
            }
            Act::Copy => self.copy_selection(),
            Act::Paste(into, how) => self.paste_into(PathBuf::from(into), how, cx),
            Act::OpenTerminal(path) => open_terminal(path),
            Act::OpenTab(id) => self.open_tab(id, window, cx),
            Act::OpenWindowAt(id) => {
                open_finder_window(id, 1, cx);
            }
            Act::AddToDock(id) => {
                if let Err(error) = crate::dock::add(&[PathBuf::from(id)]) {
                    self.complain(&error);
                }
            }
            Act::Enclosing(id) => self.show_enclosing(id, window, cx),
            Act::RemoveFromSidebar(id) => self.remove_from_sidebar(id, cx),
            Act::TagColor(name, color) => self.recolor_tag(name, color, cx),
            Act::RenameAt(id) => {
                self.show_enclosing(id, window, cx);
                self.renaming = Some(id.into());
            }
            Act::Eject(id) => self.eject(id, window, cx),
            Act::CopyPathname(id) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.to_string()))
            }
            Act::CopyPathnames => self.copy_pathnames(cx),
            Act::Slideshow => self.slideshow(window, cx),
            Act::Toggle(column) => self.toggle_column(column, cx),
            Act::Quick(act) => self.run_quick(act, cx),
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

    fn make_folder(&mut self, stem: &str, with: Vec<PathBuf>, cx: &mut Context<Self>) {
        let into = PathBuf::from(self.target());
        if !into.is_absolute() {
            return;
        }
        let op = Op::NewFolder {
            into,
            name: stem.to_owned(),
        };
        if with.is_empty() {
            self.run_op(op, After::Rename, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move { finder_fs::ops::perform(op, Conflict::KeepBoth) })
                .await;
            let Some(folder) = made.ok().and_then(|done| done.results.first().cloned()) else {
                return;
            };
            this.update(cx, |this: &mut FinderDemo, cx| {
                this.settled(Vec::new(), vec![folder.clone()], After::Rename, cx);
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

    fn with_selection(&mut self, op: fn(Vec<PathBuf>) -> Op, after: After, cx: &mut Context<Self>) {
        let items = self.paths();
        if !items.is_empty() {
            self.run_op(op(items), after, cx);
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

    pub(crate) fn icon_anchor_or(&self, fallback: Point<Pixels>) -> SurfaceAnchor {
        match self.ql_icon.bounds() {
            Some(_) => self.ql_icon.clone(),
            None => SurfaceAnchor::at(Bounds::new(fallback, size(px(1.), px(1.)))),
        }
    }

    pub(crate) fn share_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.share_urls();
        if items.is_empty() {
            return;
        }
        let at = self.icon_anchor_or(point(window.viewport_size().width / 2., px(60.)));
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
        self.show_enclosing(intern_path(&original), window, cx);
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
        let items = trash_items();
        if items.is_empty() {
            return;
        }
        if !prefs::setting_on("WarnOnEmptyTrash", true) {
            return self.erase(items, cx);
        }
        self.confirm(
            "Are you sure you want to permanently erase the items in the Trash?",
            "You can’t undo this action.",
            "Empty Trash",
            false,
            cx,
            move |this, cx| this.erase(items.clone(), cx),
        );
    }

    pub(super) fn empty_trash_now(&mut self, cx: &mut Context<Self>) {
        let items = trash_items();
        if !items.is_empty() {
            self.erase(items, cx);
        }
    }

    fn erase(&mut self, items: Vec<PathBuf>, cx: &mut Context<Self>) {
        Sound::EmptyTrash.play();
        self.run_op(Op::DeleteImmediately(items), After::Nothing, cx);
    }

    pub(crate) fn trash_selection(&mut self, cx: &mut Context<Self>) {
        let items = self.paths();
        if items.is_empty() {
            return;
        }
        if is_trash(self.here()) {
            let back: Vec<Restore> = finder_fs::put_back::origins(&items)
                .into_iter()
                .zip(items)
                .filter_map(|(to, item)| Some(Restore { item, to: to? }))
                .collect();
            if !back.is_empty() {
                self.deselect();
                self.run_op(Op::PutBack(back), After::Reveal, cx);
            }
            return;
        }
        let in_cloud = finder_fs::icloud::drive()
            .is_some_and(|drive| items.iter().all(|item| item.starts_with(&drive)));
        if !in_cloud || !prefs::setting_on("FXEnableRemoveFromICloudDriveWarning", false) {
            return self.remove(Op::Trash(items), cx);
        }
        let (asked, detail) = match items.as_slice() {
            [one] => (
                format!(
                    "Are you sure you want to delete \u{201c}{}\u{201d} from iCloud?",
                    finder_fs::name::of_path(one)
                ),
                "This item will be moved to the Trash on this Mac and deleted from iCloud\u{a0}Drive and your other iCloud devices.".to_owned(),
            ),
            many => (
                format!(
                    "Are you sure you want to delete these {} items from iCloud?",
                    many.len()
                ),
                format!(
                    "These {} items will be moved to the Trash on this Mac and deleted from iCloud\u{a0}Drive and your other iCloud devices.",
                    many.len()
                ),
            ),
        };
        self.confirm(&asked, &detail, "Delete", false, cx, move |this, cx| {
            this.remove(Op::Trash(items.clone()), cx)
        });
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
        self.confirm(&text, &detail, "Delete", true, cx, move |this, cx| {
            this.remove(Op::DeleteImmediately(items.clone()), cx)
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
        let start = self.listing_start(id);
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

    pub(crate) fn open_new_window(&self, cx: &mut Context<Self>) {
        let target = new_window_target();
        let window = open_finder_window(self.listing_start(target), 1, cx);
        if target == RECENTS {
            window
                .update(cx, |demo, window, cx| demo.show_tag(target, window, cx))
                .ok();
        }
    }

    fn listing_start(&self, id: &'static str) -> &'static str {
        match id.starts_with("tag:") || id == RECENTS {
            true => self.here(),
            false => id,
        }
    }
}

pub(super) fn open_windows(ids: impl IntoIterator<Item = &'static str>, cx: &mut App) {
    for (extra, id) in ids.into_iter().enumerate() {
        open_finder_window(id, extra + 1, cx);
    }
}

/// The window New Finder Window opens when no window is left to ask, at home for a listing.
pub(crate) fn open_first_window(cx: &mut App) -> WindowHandle<FinderDemo> {
    let target = new_window_target();
    let listing = target.starts_with("tag:") || target == RECENTS;
    let start = match listing {
        true => intern_path(&home()),
        false => target,
    };
    let window = open_finder_window(start, 0, cx);
    if listing {
        window
            .update(cx, |demo, window, cx| demo.show_tag(target, window, cx))
            .ok();
    }
    window
}
