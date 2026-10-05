use std::ffi::CString;
use std::ops::Range;

use finder_fs::sort::fold;
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, FontWeight, MouseButton, ScrollStrategy,
    SharedString, UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowOptions, div,
    point, prelude::*, px, size, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{
    Blending, Layer, Material, begin_sheet, material, native_label, native_path_control,
    native_root, native_rule, native_symbol, system_corner_radius, window_options,
};
use objc2::encode::Encode;
use objc2::runtime::{AnyClass, Imp, Sel};
use objc2::{ClassType, msg_send, sel};
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::frame::ns_window;
use crate::navigate::{folder_at_path, typed_path};
use crate::prefs;
use crate::ui::{focus_gpui, key_to_parent, native_text_field, text_width};
use crate::{home, index};

const SHEET_W: f32 = 520.;
const PANEL_W: f32 = 460.;
const PANEL_H: f32 = 183.;
const MIN_W: f32 = 434.;
const MAX_W: f64 = 854.;
const MAX_H: f64 = 345.;
const BELOW_TOOLBAR: f64 = 50.;
const FIELD_X: f32 = 15.;
const FIELD_Y: f32 = 7.;
const FIELD_H: f32 = 22.;
const FIELD_FONT: f32 = 15.;
const CLEAR: f32 = 18.;
const GHOST_RIGHT: f32 = 35.;
const FIELD_TAIL: f32 = 64.;
const SEPARATOR_Y: f32 = 34.5;
const LIST_Y: f32 = 36.;
const LIST_PAD: f32 = 10.;
const ROW_H: f32 = 22.;
const HEADING_X: f32 = 17.;
const PATH_X: f32 = 15.;
const PATH_RIGHT: f32 = 11.;
const HEADING_FONT: f32 = 12.;
const CLEAR_GLYPH: f32 = 14.;
/// Our field is 29pt tall to Finder's 22, which sets its text this much higher.
const FIELD_DROP: f32 = 3.5;
const PICK_INSET: f32 = 10.;
const PICK_RADIUS: f32 = 8.;
/// A borderless field sets its line above centre; a centred label does not.
const GHOST_LIFT: f32 = 7.;
const CHOICES: usize = 200;
const HISTORY: usize = 8;
const COMMANDS: &[&str] = &[
    "moveUp:",
    "moveDown:",
    "moveRight:",
    "insertTab:",
    "deleteWordBackward:",
];

enum GoLine {
    Heading(&'static str),
    Path(String),
}

pub(crate) struct GoPanel {
    demo: WeakEntity<FinderDemo>,
    host: AnyWindowHandle,
    here: &'static str,
    typed: String,
    lines: Vec<GoLine>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    unshown: Option<AnyWindowHandle>,
}

impl GoPanel {
    fn chosen(&self) -> Option<&str> {
        match self.lines.get(self.selected?)? {
            GoLine::Path(path) => Some(path),
            GoLine::Heading(_) => None,
        }
    }
}

fn expanded(typed: &str) -> String {
    match typed.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", home().to_string_lossy()),
        None => typed.to_string(),
    }
}

fn go_rows(typed: &str) -> Vec<String> {
    let folder = typed
        .ends_with('/')
        .then(|| typed_path(typed))
        .filter(|path| finder_fs::list::is_dir(path))
        .map(|path| path.to_string_lossy().into_owned());
    folder.into_iter().chain(completions(typed)).collect()
}

fn completions(typed: &str) -> impl Iterator<Item = String> {
    finder_fs::complete::complete(typed, Some(index()), CHOICES)
        .into_iter()
        .map(|found| found.path.to_string_lossy().into_owned())
}

fn drop_folder(typed: &str) -> String {
    let trimmed = typed.strip_suffix('/').unwrap_or(typed);
    match trimmed.rfind('/') {
        Some(at) => typed[..=at].to_string(),
        None => String::new(),
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("/")
}

fn ghost(sheet: &GoPanel) -> Option<String> {
    let chosen = sheet.chosen().filter(|_| !sheet.typed.is_empty())?;
    let typed = expanded(&sheet.typed);
    if typed.trim_end_matches('/') == chosen {
        return None;
    }
    let rest = chosen
        .get(..typed.len())
        .filter(|head| fold(head) == fold(&typed))
        .and_then(|_| chosen.get(typed.len()..));
    match rest {
        Some(rest) => (!rest.is_empty()).then(|| rest.to_string()),
        None => Some(format!(" \u{2013} {}", leaf(chosen))),
    }
}

/// Which of the leaf's characters spell out `typed` in order: the ones Finder sets in bold.
fn marks(typed: &str, leaf: &str) -> Option<Vec<usize>> {
    let mut wanted = typed.chars().flat_map(char::to_lowercase).peekable();
    let mut hits = Vec::new();
    for (at, ch) in leaf.chars().enumerate() {
        let Some(&next) = wanted.peek() else {
            break;
        };
        if ch.to_lowercase().eq(std::iter::once(next)) {
            hits.push(at);
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(hits)
}

fn section(heading: &'static str, paths: Vec<String>) -> Vec<GoLine> {
    if paths.is_empty() {
        return Vec::new();
    }
    std::iter::once(GoLine::Heading(heading))
        .chain(paths.into_iter().map(GoLine::Path))
        .collect()
}

/// A bare name lists matches here and among recents; a path, or a name found in neither, completes.
fn go_lines(typed: &str, here: &str) -> Vec<GoLine> {
    let bare = !typed.contains('/') && !typed.starts_with('~');
    if !bare {
        return section("Go to:", go_rows(typed));
    }
    let nearby: Vec<String> = if here.starts_with('/') {
        completions(&format!("{}/{typed}", here.trim_end_matches('/'))).collect()
    } else {
        Vec::new()
    };
    let recent: Vec<String> = prefs::go_history()
        .into_iter()
        .filter(|path| !nearby.contains(path) && marks(typed, leaf(path)).is_some())
        .collect();
    if nearby.is_empty() && recent.is_empty() {
        return section("Go to:", go_rows(typed));
    }
    let mut lines = section("Go to folder in current location", nearby);
    lines.extend(section("Recents", recent));
    lines
}

fn go_heading(at: usize, heading: &'static str) -> AnyElement {
    div()
        .id(("ng.finder.go.heading", at))
        .w_full()
        .h(px(ROW_H))
        .pl(px(HEADING_X))
        .flex()
        .items_center()
        .child(
            native_label(("ng.finder.go.heading.label", at), heading)
                .font_size(px(HEADING_FONT))
                .weight(FontWeight::SEMIBOLD)
                .color(SystemColor::TertiaryLabel),
        )
        .into_any_element()
}

fn go_row(
    at: usize,
    path: &str,
    picked: bool,
    bold: Vec<usize>,
    go: WeakEntity<GoPanel>,
) -> AnyElement {
    let mut titles: Vec<SharedString> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| SharedString::from(part.to_string()))
        .collect();
    if titles.is_empty() {
        titles.push("/".into());
    }
    let clicked = go.clone();
    div()
        .id(("ng.finder.go.row", at))
        .relative()
        .h(px(ROW_H))
        .when(picked, |row| {
            row.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(PICK_INSET))
                    .right(px(PICK_INSET))
                    .child(
                        // Finder's panel keeps the accent highlight while its field has focus.
                        native_rule("ng.finder.go.pick")
                            .tint(tokens::system_color(SystemColor::SelectedContentBackground))
                            .corner_radius(PICK_RADIUS),
                    ),
            )
        })
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(PATH_X))
                .right(px(PATH_RIGHT))
                .h(px(ROW_H))
                .flex()
                .items_center()
                .child(
                    native_path_control(("ng.finder.go.path", at), path)
                        .titles(titles)
                        .bold(bold)
                        .emphasized(picked)
                        .on_click(move |_, window, cx| pick(&clicked, at, window, cx)),
                ),
        )
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            pick(&go, at, window, cx)
        })
        .into_any_element()
}

fn pick(go: &WeakEntity<GoPanel>, at: usize, window: &mut Window, cx: &mut App) {
    go.update(cx, |panel: &mut GoPanel, cx| {
        panel.selected = Some(at);
        panel.submit("", window, cx);
    })
    .ok();
}

fn remember_go(path: &str) {
    let mut history = prefs::go_history();
    history.retain(|seen| seen != path);
    history.insert(0, path.to_string());
    history.truncate(HISTORY);
    prefs::set_go_history(&history.iter().map(String::as_str).collect::<Vec<_>>());
}

impl FinderDemo {
    pub(crate) fn open_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.go_sheet.is_some() {
            return;
        }
        let (width, height) = prefs::go_sheet_size().unwrap_or((PANEL_W, PANEL_H));
        let (demo, handle, here) = (cx.entity().downgrade(), window.window_handle(), self.here());
        let typed = prefs::go_field().unwrap_or_default();
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(width), px(height)),
                })),
                show: false,
                is_minimizable: false,
                window_min_size: Some(size(px(MIN_W), px(PANEL_H))),
                ..window_options()
            },
            |window, cx| {
                super::toolbar::system_corners(window);
                let unshown = Some(window.window_handle());
                cx.new(|cx| {
                    let mut panel = GoPanel {
                        demo,
                        host: handle,
                        here,
                        typed: String::new(),
                        lines: Vec::new(),
                        selected: None,
                        scroll: UniformListScrollHandle::new(),
                        unshown,
                    };
                    panel.typing(typed, cx);
                    panel
                })
            },
        );
        let Ok(opened) = opened else {
            return;
        };
        let Ok(Some(sheet)) = opened.update(cx, |_, window, _| ns_window(window)) else {
            return;
        };
        sheet.setContentMaxSize(NSSize::new(MAX_W, MAX_H));
        sheet.setFrameAutosaveName(&NSString::from_str(prefs::GO_SHEET));
        hang_below_toolbar(&sheet);
        self.go_sheet = Some(opened);
    }
}

/// AppKit slides a sheet down and dims its window only when it centres it; Finder moves that frame.
fn hang_below_toolbar(sheet: &NSWindow) {
    type Place = extern "C" fn(&NSWindow, Sel, NSRect, &NSWindow, NSRect) -> NSRect;
    let place: Place = place_sheet;
    let types = CString::new(format!(
        "{}@:{}@{}",
        NSRect::ENCODING,
        NSRect::ENCODING,
        NSRect::ENCODING
    ))
    .expect("no NUL");
    unsafe {
        objc2::ffi::class_addMethod(
            sheet.class() as *const AnyClass as *mut AnyClass,
            sel!(_centerSheetFrame:forParent:inParentFrame:),
            std::mem::transmute::<Place, Imp>(place),
            types.as_ptr(),
        );
    }
}

extern "C" fn place_sheet(
    this: &NSWindow,
    _: Sel,
    frame: NSRect,
    parent: &NSWindow,
    below_toolbar: NSRect,
) -> NSRect {
    let centred: NSRect = unsafe {
        msg_send![
            super(this, NSWindow::class()),
            _centerSheetFrame: frame,
            forParent: parent,
            inParentFrame: below_toolbar
        ]
    };
    if this.frameAutosaveName().to_string() != prefs::GO_SHEET {
        return centred;
    }
    let top = below_toolbar.origin.y + below_toolbar.size.height - BELOW_TOOLBAR;
    NSRect::new(
        NSPoint::new(centred.origin.x, top - centred.size.height),
        centred.size,
    )
}

impl GoPanel {
    fn typing(&mut self, text: String, cx: &mut Context<Self>) {
        self.typed = text.clone();
        cx.notify();
        if text.is_empty() {
            self.lines = section("Recents", prefs::go_history());
            self.selected = None;
            self.show(cx);
            return;
        }
        let here = self.here;
        cx.spawn(async move |this, cx| {
            let wanted = text.clone();
            let lines = cx
                .background_executor()
                .spawn(async move { go_lines(&text, here) })
                .await;
            this.update(cx, |this, cx| {
                if this.typed != wanted {
                    return;
                }
                this.selected = lines
                    .iter()
                    .position(|line| matches!(line, GoLine::Path(_)));
                this.lines = lines;
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
                this.show(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Finder slides its sheet in once the rows it opens with are in.
    fn show(&mut self, cx: &mut Context<Self>) {
        if let Some(sheet) = self.unshown.take() {
            begin_sheet(self.host, sheet, cx);
        }
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let paths: Vec<usize> = (0..self.lines.len())
            .filter(|&at| matches!(self.lines[at], GoLine::Path(_)))
            .collect();
        let Some(last) = paths.len().checked_sub(1) else {
            return;
        };
        let now = self
            .selected
            .and_then(|selected| paths.iter().position(|&at| at == selected));
        let step = match now {
            None if by > 0 => 0,
            None => return,
            Some(step) => step.saturating_add_signed(by).min(last),
        };
        let at = paths[step];
        self.selected = Some(at);
        // The first path brings its heading into view with it.
        match step {
            0 => self.scroll.scroll_to_item(0, ScrollStrategy::Top),
            _ => self.scroll.scroll_to_item(at, ScrollStrategy::Nearest),
        }
        cx.notify();
    }

    fn accept(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.chosen() else {
            return;
        };
        let text = format!("{}/", chosen.trim_end_matches('/'));
        self.typing(text, cx);
    }

    /// Off the main thread: a path on a volume that stopped answering must not hang the window.
    fn submit(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self
            .chosen()
            .map_or_else(|| typed.to_string(), str::to_string);
        if typed.trim().is_empty() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let (folder, item, refused) = cx
                .background_executor()
                .spawn(async move {
                    let folder = folder_at_path(&typed);
                    let path = typed_path(&typed);
                    // Finder leaves the sheet up, and says nothing, on a folder it can't list.
                    let refused = folder.is_none() && path.is_dir();
                    let item = (folder.is_none() && !refused && path.symlink_metadata().is_ok())
                        .then(|| intern_path(&path));
                    if folder.is_some() || item.is_some() {
                        remember_go(&path.to_string_lossy());
                    }
                    (folder, item, refused)
                })
                .await;
            if refused {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.close(window, cx, move |demo, window, cx| match (folder, item) {
                    (Some(id), _) => demo.go(id, window, cx),
                    (None, Some(item)) => demo.show_enclosing(item, window, cx),
                    (None, None) => {}
                })
            })
            .ok();
        })
        .detach();
    }

    fn close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut FinderDemo, &mut Window, &mut Context<FinderDemo>) + 'static,
    ) {
        let (demo, host, handle) = (self.demo.clone(), self.host, window.window_handle());
        let sheet = ns_window(window);
        cx.spawn(async move |_, cx| {
            if let Some(sheet) = sheet {
                if let Some(parent) = sheet.sheetParent() {
                    parent.endSheet(&sheet);
                }
            }
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            host.update(cx, |_, window, cx| {
                demo.update(cx, |demo, cx| {
                    demo.go_sheet = None;
                    then(demo, window, cx);
                })
            })
            .ok();
        })
        .detach();
    }
}

impl Render for GoPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let field = native_text_field("ng.finder.go", &self.typed)
            .stretch()
            .borderless()
            .font_size(FIELD_FONT)
            .placeholder("Go to Folder")
            .editing(0..self.typed.chars().count())
            .on_change(cx.listener(|panel, text: &String, _, cx| panel.typing(text.clone(), cx)))
            .on_command(COMMANDS, move |command, text, _, cx| {
                this.update(cx, |panel: &mut GoPanel, cx| match command {
                    "moveUp:" => panel.step(-1, cx),
                    "moveDown:" => panel.step(1, cx),
                    "deleteWordBackward:" => panel.typing(drop_folder(text), cx),
                    _ => panel.accept(cx),
                })
                .ok();
            })
            .on_submit(
                cx.listener(|panel, typed: &String, window, cx| panel.submit(typed, window, cx)),
            )
            .on_cancel(
                cx.listener(|panel, _: &String, window, cx| panel.close(window, cx, |_, _, _| {})),
            );

        let typed_w = text_width(window, self.typed.clone(), px(FIELD_FONT));
        let ghost = ghost(self).map(|ghost| {
            div()
                .absolute()
                .top_0()
                .bottom(px(GHOST_LIFT))
                // The label insets its text as the field does, so the two insets cancel.
                .left(px(typed_w))
                .right_0()
                .flex()
                .items_center()
                .child(
                    native_label("ng.finder.go.ghost", &ghost)
                        .font_size(px(FIELD_FONT))
                        .color(SystemColor::TertiaryLabel),
                )
        });
        let clear = div()
            .id("ng.finder.go.clear")
            .absolute()
            .top(px(FIELD_Y + (FIELD_H - CLEAR) / 2.))
            .right(px(FIELD_X - 1.))
            .size(px(CLEAR))
            .flex()
            .items_center()
            .justify_center()
            .child(
                native_symbol(
                    "ng.finder.go.clear.glyph",
                    "xmark.circle.fill",
                    CLEAR_GLYPH,
                    FontWeight::NORMAL,
                )
                .tint(SystemColor::SecondaryLabel),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|panel, _, _, cx| panel.typing(String::new(), cx)),
            );
        let entry = div()
            .absolute()
            .top(px(FIELD_Y + FIELD_DROP))
            .left(px(FIELD_X))
            .right(px(GHOST_RIGHT))
            .h(px(FIELD_H))
            .flex()
            .items_center()
            .overflow_hidden()
            .children(ghost)
            .child(div().w_full().pr(px(FIELD_TAIL)).child(field));

        let list = uniform_list(
            "ng.finder.go.rows",
            self.lines.len(),
            cx.processor(|panel, range: Range<usize>, _, cx| {
                let this = cx.entity().downgrade();
                let tail = panel.typed.rsplit('/').next().unwrap_or_default();
                range
                    .map(|at| match &panel.lines[at] {
                        GoLine::Heading(heading) => go_heading(at, heading),
                        GoLine::Path(path) => {
                            let bold = marks(tail, leaf(path)).unwrap_or_default();
                            go_row(at, path, panel.selected == Some(at), bold, this.clone())
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full()
        .track_scroll(&self.scroll);

        let radius = f32::from(system_corner_radius(window));
        let white = tokens::system_color(SystemColor::ControlBackground);
        // The list's fill laid over the white, opaque so the rules below may overlap.
        let gray = white.blend(tokens::system_color(SystemColor::QuaternarySystemFill));
        let band = |top: f32, height: f32| {
            div()
                .absolute()
                .top(px(top))
                .h(px(height))
                .left_0()
                .right_0()
        };
        // Gray fills the sheet; the white band takes its top corners, a gray strip squares its bottom.
        native_root(
            div()
                .relative()
                .size_full()
                .child(
                    div().absolute().inset_0().child(
                        native_rule("ng.finder.go.list")
                            .tint(gray)
                            .corner_radius(radius),
                    ),
                )
                .child(
                    band(0., LIST_Y + radius).child(
                        native_rule("ng.finder.go.band")
                            .tint(white)
                            .corner_radius(radius),
                    ),
                )
                .child(band(LIST_Y, radius).child(native_rule("ng.finder.go.square").tint(gray)))
                .child(
                    band(SEPARATOR_Y, 1.).child(
                        native_rule("ng.finder.go.separator")
                            .tint(tokens::system_color(SystemColor::Separator)),
                    ),
                )
                .child(entry)
                .child(clear)
                .child(
                    div()
                        .absolute()
                        .top(px(LIST_Y + LIST_PAD))
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .child(list),
                ),
        )
    }
}

impl FinderDemo {
    pub(crate) fn save_sheet(&self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let field = native_text_field("ng.finder.save", name)
            .stretch()
            .editing(0..name.chars().count())
            .on_submit(cx.listener(|demo, typed: &String, window, cx| {
                demo.end_save_sheet(window, cx);
                let typed = typed.trim();
                if !typed.is_empty() {
                    demo.save_search(typed, window, cx);
                }
            }))
            .on_cancel(cx.listener(|demo, _: &String, window, cx| demo.end_save_sheet(window, cx)));
        let corner = px(super::toolbar::SYSTEM_RADIUS as f32);
        div()
            .absolute()
            .top(self.metrics().top)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(SHEET_W))
                    .rounded(corner)
                    .overflow_hidden()
                    .shadow_lg()
                    .block_mouse_except_scroll()
                    // Native and Above, like the Go panel: the header band covers all of GPUI.
                    .child(
                        material("ng.finder.save.glass", Material::Popover)
                            .layer(Layer::Above)
                            .blending(Blending::WithinWindow)
                            .corner_radius(corner)
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .px(px(20.))
                            .py(px(18.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                native_label(("ng.finder.save.glass", 1usize), "Save As:")
                                    .font_size(px(13.))
                                    .weight(FontWeight::SEMIBOLD),
                            )
                            .child(div().h(px(24.)).child(field)),
                    ),
            )
            .into_any_element()
    }

    fn end_save_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_sheet = None;
        focus_gpui(window);
        key_to_parent(window);
        cx.notify();
    }
}
