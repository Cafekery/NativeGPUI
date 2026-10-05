use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gpui::{
    AnyElement, ClickEvent, Context, Div, DragMoveEvent, ElementId, ExternalPaths, FontWeight,
    Hsla, Priority, SharedString, Stateful, Window, div, hsla, prelude::*, px,
};
use native_gpui::tokens::{self, Appearance, FileType, SystemColor};
use native_gpui::{native_label, native_raster, native_rule, native_symbol};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing, NSBundleImageExtension, NSFont, NSFontAttributeName,
};
use objc2_foundation::{NSAttributedString, NSBundle, NSDictionary, NSString};

use crate::app::FinderDemo;
use crate::data::{Line, folder_artwork, icloud_drive, is_volume_root, split_key};
use crate::dragdrop::{Grabbed, RowDrag};
use crate::locations::{SHARED, is_location};
use crate::menus;
use crate::palette::Palette;
use crate::search::RECENTS;
use crate::view::COLUMN_VIEW;

pub(crate) const PATH_H: f32 = 28.;

const TRAIL_INSET: f32 = 6.;

const TRAIL_ROOM: f32 = 16.;

const ICON_LEAD: f32 = 5.;

const CRUMB_ICON: f32 = 16.;

// Finder's small NSPathControl draws each component's image in a 14pt square.
pub(crate) const CRUMB_ART: f32 = 14.;

// An NSTextField label draws its text 2pt inside its frame.
pub(crate) const LABEL_INSET: f32 = 2.;

const LABEL_LEAD: f32 = 1.;

const CHEVRON_LEAD: f32 = 1.;

const CRUMB_CHROME: f32 = 29.;

const LAST_CHROME: f32 = 24.;

const DROP_RADIUS: f32 = 7.;

pub(crate) const EDGE_ROW: f32 = 0.5;

fn is_alt_click(event: &ClickEvent) -> bool {
    matches!(event, ClickEvent::Mouse(mouse) if mouse.up.modifiers.alt)
}

// No stand-in: Finder's first frame already has each crumb's own icon.
fn crumb_art(id: impl Into<ElementId>, kind: Option<FileType<'static>>) -> AnyElement {
    let raster = kind.and_then(|kind| crate::icons::icon(kind, CRUMB_ART));
    div()
        .size(px(CRUMB_ICON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .children(raster.map(|raster| native_raster(id, raster)))
        .into_any_element()
}

const KEPT: usize = 256;

type Art = HashMap<(&'static str, bool, bool), FileType<'static>>;

type Attributes = Retained<NSDictionary<NSString, AnyObject>>;

thread_local! {
    static LABEL_W: RefCell<HashMap<String, f32>> = RefCell::default();
    static LABEL_ATTRS: RefCell<Option<(f64, Attributes)>> = RefCell::default();
    static ART: RefCell<(u64, Art)> = RefCell::default();
}

// Finder's path bar draws a folder as `iconForFile:` gives it: plain, whatever it holds.
fn plain(kind: FileType<'static>) -> FileType<'static> {
    match kind {
        FileType::FullFolder => FileType::Folder,
        FileType::FullPath(path) => FileType::Path(path),
        kind => kind,
    }
}

fn crumb_artwork(path: &'static str, holds: bool, landed: u64) -> FileType<'static> {
    let custom = crate::custom::custom_icon(&SharedString::new_static(path));
    ART.with_borrow_mut(|(at, kept)| {
        if *at != landed || kept.len() >= KEPT {
            *at = landed;
            kept.clear();
        }
        *kept
            .entry((path, holds, custom))
            .or_insert_with(|| plain(folder_artwork(path, holds)))
    })
}

const ICLOUD_ART: &str = "iCloudDriveNode";

pub(crate) fn icloud_art() -> FileType<'static> {
    thread_local! {
        static NAMED: Cell<bool> = const { Cell::new(false) };
    }
    if !NAMED.replace(true) {
        let finder = NSBundle::bundleWithPath(&NSString::from_str(
            "/System/Library/CoreServices/Finder.app",
        ));
        let name = NSString::from_str(ICLOUD_ART);
        if let Some(art) = finder.and_then(|finder| finder.imageForResource(&name)) {
            art.setName(Some(&name));
        }
    }
    FileType::Named(ICLOUD_ART)
}

fn crumb_kind(index: usize, path: &'static str, holds: bool, landed: u64) -> FileType<'static> {
    if index > 0 || crate::data::is_trash(path) {
        crumb_artwork(path, holds, landed)
    } else if icloud_drive() == Some(path) {
        icloud_art()
    } else {
        crate::locations::artwork(path).unwrap_or(FileType::Path(path))
    }
}

fn takes_drop<D: Grabbed + 'static>(
    crumb: Stateful<Div>,
    index: usize,
    into: &'static str,
    cx: &mut Context<FinderDemo>,
) -> Stateful<Div> {
    crumb
        .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<D>, _, cx| {
            this.lit_crumb(index, event.bounds.contains(&event.event.position), cx);
        }))
        .on_drop(cx.listener(move |this, grab: &D, window: &mut Window, cx| {
            this.crumb_dropped(into, grab, window, cx);
        }))
}

/// The frame an `NSTextField` label needs to show `text` whole, kerned as AppKit sets it.
fn label_width(text: &str) -> f32 {
    LABEL_W.with(|kept| {
        if let Some(width) = kept.borrow().get(text) {
            return *width;
        }
        let size = f64::from(f32::from(tokens::small_system_font_size()));
        let attrs = LABEL_ATTRS.with_borrow_mut(|held| match held {
            Some((at, attrs)) if *at == size => attrs.clone(),
            _ => {
                let font = NSFont::systemFontOfSize(size);
                let font: &AnyObject = font.as_ref();
                let attrs = NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[font]);
                held.insert((size, attrs)).1.clone()
            }
        });
        let string = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attrs),
            )
        };
        let width = string.size().width.ceil() as f32 + 2. * LABEL_INSET;
        let mut kept = kept.borrow_mut();
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(text.to_owned(), width);
        width
    })
}

impl FinderDemo {
    /// Every art `id`'s crumbs may draw: the Trash's full and empty both.
    pub(crate) fn crumb_arts(&self, id: &'static str) -> Vec<FileType<'static>> {
        let landed = self.folders.changed();
        let crumbs = self
            .folders
            .get(id)
            .map_or(&[][..], |folder| folder.crumbs.as_slice());
        let mut arts: Vec<_> = crumbs
            .iter()
            .enumerate()
            .map(|(index, &(_, path))| crumb_kind(index, path, true, landed))
            .collect();
        if let Some(&(_, path)) = crumbs.last() {
            arts.push(crumb_kind(crumbs.len() - 1, path, false, landed));
        }
        arts
    }

    pub(crate) fn draw_crumbs(&self, id: &'static str, cx: &mut Context<Self>) {
        let arts = self.crumb_arts(id);
        let drawn = cx
            .background_executor()
            .spawn_with_priority(Priority::High, async move {
                crate::icons::begin(arts, CRUMB_ART).finish();
            });
        cx.spawn(async move |this, cx| {
            drawn.await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    pub(crate) fn path_bar(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lone = self.lone_pick();
        let holder = lone
            .as_ref()
            .and_then(|line| split_key(&line.key))
            .map_or(self.target(), |(dir, _)| dir);
        let placed = match &lone {
            Some(line) => !line
                .row
                .opens
                .is_some_and(|id| is_volume_root(id) || is_location(id)),
            None => self.here() != SHARED && self.tag_listing != Some(RECENTS),
        };
        let here = self.folders.get(holder).filter(|_| placed);
        let (mut crumbs, mut targets): (Vec<SharedString>, Vec<Option<&'static str>>) = here
            .map_or(&[][..], |folder| folder.crumbs.as_slice())
            .iter()
            .map(|&(name, path)| (SharedString::new_static(name), Some(path)))
            .unzip();
        let listed = here.is_some_and(|folder| !folder.rows.is_empty());
        let settled = listed || here.is_some_and(|folder| !folder.loading);
        if let Some(line) = &lone {
            crumbs.push(line.row.display.clone());
            targets.push(line.row.opens);
        }
        let widths = self.crumb_widths(&crumbs);
        let lit = self.crumb_drop.filter(|_| cx.has_active_drag());
        let landed = self.folders.changed();
        let mut trail = div().size_full().flex().items_center().pl(px(TRAIL_INSET));

        for (index, (crumb, target)) in crumbs.iter().zip(targets).enumerate() {
            let last = index + 1 == crumbs.len();
            let kind = match &lone {
                Some(line) if last => {
                    Some(plain(line.row.artwork.unwrap_or(line.row.face.stand_in())))
                }
                _ => target.and_then(|path| {
                    let kind = crumb_kind(index, path, !last || listed, landed);
                    // The Trash's can follows what it holds: none until its first rows say which.
                    (settled || kind == crumb_kind(index, path, true, landed)).then_some(kind)
                }),
            };
            let mut segment = div()
                .id(("ng.finder.crumb", index))
                .h_full()
                .flex()
                .items_center()
                .pl(px(ICON_LEAD))
                .child(crumb_art(("ng.finder.crumb.art", index), kind));
            if let Some(width) = widths[index] {
                let dropping = lit == Some(index);
                segment = segment.child(
                    div()
                        .relative()
                        .flex_none()
                        .ml(px(LABEL_LEAD))
                        .w(px(width))
                        .when(dropping, |name| {
                            name.child(
                                div().absolute().inset_0().child(
                                    native_rule(("ng.finder.crumb.drop", index))
                                        .tint(palette.selection)
                                        .corner_radius(DROP_RADIUS),
                                ),
                            )
                        })
                        // A hosted view whose frame changes costs AppKit a window layout pass.
                        .child(
                            div()
                                .w(px(if last {
                                    width.max(self.list_room.floor())
                                } else {
                                    width
                                }))
                                .child(
                                    native_label(("ng.finder.crumb.name", index), crumb)
                                        .font_size(tokens::small_system_font_size())
                                        .truncates(false)
                                        .color(if dropping {
                                            SystemColor::AlternateSelectedControlText
                                        } else {
                                            SystemColor::SecondaryLabel
                                        }),
                                ),
                        ),
                );
            }
            if !last {
                segment = segment.child(
                    div().flex_none().ml(px(CHEVRON_LEAD)).child(
                        native_symbol(
                            ("ng.finder.crumb.chevron", index),
                            "chevron.right",
                            6.5,
                            FontWeight::EXTRA_BOLD,
                        )
                        .tint(SystemColor::SecondaryLabel),
                    ),
                );
            }
            let segment = segment.when_some(target, |crumb_row, id| {
                let crumb_row =
                    crumb_row.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if is_alt_click(event) {
                            crate::open_finder_window(id, 1, cx);
                            window.remove_window();
                        } else {
                            this.go(id, window, cx);
                        }
                    }));
                let crumb_row = takes_drop::<RowDrag>(crumb_row, index, id, cx);
                takes_drop::<ExternalPaths>(crumb_row, index, id, cx)
            });
            trail = trail.child(match target {
                Some(id) => menus::crumb_menu(
                    ("ng.finder.crumb.menu", index),
                    id,
                    crumb.clone(),
                    cx.entity().downgrade(),
                    segment,
                )
                .into_any_element(),
                None => segment.into_any_element(),
            });
        }

        let paint = BarPaint::now();
        div()
            .h(px(PATH_H))
            .flex_none()
            .relative()
            .block_mouse_except_scroll()
            .bg(paint.lift)
            .child(edge_row(0., paint.rim[0]))
            .child(edge_row(EDGE_ROW, paint.rim[1]))
            .child(trail)
            .into_any_element()
    }

    fn lit_crumb(&mut self, index: usize, over: bool, cx: &mut Context<Self>) {
        let lit = if over {
            Some(index)
        } else if self.crumb_drop == Some(index) {
            None
        } else {
            return;
        };
        if self.crumb_drop != lit {
            self.crumb_drop = lit;
            cx.notify();
        }
    }

    fn crumb_dropped<D: Grabbed>(
        &mut self,
        into: &'static str,
        grab: &D,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.crumb_drop = None;
        if !grab.wandered(window, self.pressed_at) {
            return;
        }
        let keys = grab.keys(&self.selected.snapshot());
        self.drop_into(into, &keys, window, cx);
        cx.notify();
    }

    fn lone_pick(&self) -> Option<Line> {
        if self.selected.len() != 1 {
            return None;
        }
        let line = match self.view {
            COLUMN_VIEW => self.column_chain().1,
            _ => self.cursor_line(),
        };
        line.filter(|line| self.selected.contains(&line.key))
    }

    /// `None` where only the icon fits; the widest middle crumbs share one cap until the trail fits.
    fn crumb_widths(&self, crumbs: &[SharedString]) -> Vec<Option<f32>> {
        let labels: Vec<f32> = crumbs.iter().map(|crumb| label_width(crumb)).collect();
        let Some(last) = labels.len().checked_sub(1).filter(|last| *last > 1) else {
            return labels.into_iter().map(Some).collect();
        };
        let room = self.list_room - TRAIL_ROOM;
        let fixed = labels[0] + CRUMB_CHROME + labels[last] + LAST_CHROME;
        let mut middle: Vec<f32> = labels[1..last]
            .iter()
            .map(|label| label + CRUMB_CHROME)
            .collect();
        if fixed + middle.iter().sum::<f32>() <= room {
            return labels.into_iter().map(Some).collect();
        }
        middle.sort_by(f32::total_cmp);
        let mut spare = room - fixed;
        let mut cap = 0.;
        for (at, item) in middle.iter().enumerate() {
            let rest = (middle.len() - at) as f32;
            if item * rest > spare {
                cap = (spare / rest * self.scale).ceil() / self.scale;
                break;
            }
            spare -= item;
        }
        labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                if index == 0 || index == last {
                    return Some(*label);
                }
                let label = (label + CRUMB_CHROME).min(cap) - CRUMB_CHROME;
                (label > 2. * LABEL_INSET).then_some(label)
            })
            .collect()
    }
}

pub(crate) fn edge_row(top: f32, color: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .top(px(top))
        .left_0()
        .right_0()
        .h(px(EDGE_ROW))
        .bg(color)
}

/// Finder's bottom bars have no separator view; their edges and fills are colours no `NSColor` names.
pub(crate) struct BarPaint {
    pub(crate) lift: Hsla,
    pub(crate) rim: [Hsla; 2],
    pub(crate) floor: Hsla,
    pub(crate) rule: [Hsla; 2],
}

impl BarPaint {
    pub(crate) fn now() -> Self {
        match tokens::current_appearance() {
            Appearance::Dark => Self {
                lift: hsla(0., 0., 1., 0.04),
                rim: [hsla(0., 0., 1., 0.16), hsla(0., 0., 1., 0.068)],
                floor: hsla(0., 0., 1., 0.037),
                rule: [hsla(0., 0., 0., 0.27), hsla(0., 0., 0., 1.)],
            },
            Appearance::Light => Self {
                lift: hsla(0., 0., 1., 0.04),
                rim: [hsla(0., 0., 0., 0.129), hsla(0., 0., 0., 0.055)],
                floor: hsla(0., 0., 1., 0.04),
                rule: [hsla(0., 0., 0., 0.051), hsla(0., 0., 0., 0.145)],
            },
        }
    }
}
