use finder_fs::sort::fold;
use gpui::{
    AnyElement, App, Context, FontWeight, MouseButton, Pixels, ScrollStrategy, SharedString,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, uniform_list,
};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{
    Blending, Layer, Material, material, native_label, native_path_control, native_rule,
    native_symbol, system_corner_radius,
};

use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::navigate::{folder_at_path, typed_path};
use crate::prefs;
use crate::ui::{focus_gpui, key_to_parent, native_text_field, text_width};
use crate::{home, index};

const SHEET_W: f32 = 520.;
const PANEL_TOP: f32 = 102.;
// Finder's Go to Folder window, measured in points.
const PANEL_W: f32 = 460.;
const PANEL_H: f32 = 183.;
const FIELD_X: f32 = 15.;
const FIELD_Y: f32 = 7.;
const FIELD_H: f32 = 22.;
const FIELD_FONT: f32 = 15.;
const CLEAR: f32 = 18.;
const SEPARATOR_Y: f32 = 34.5;
const LIST_Y: f32 = 36.;
const LIST_PAD: f32 = 10.;
const ROW_H: f32 = 22.;
const HEADING_X: f32 = 17.;
const PATH_X: f32 = 15.;
const PATH_W: f32 = 434.;
const HEADING_FONT: f32 = 12.;
const CLEAR_GLYPH: f32 = 14.;
/// Our field fits 29pt tall where Finder's is 22; measured, its text then sits 3.5pt high.
const FIELD_DROP: f32 = 3.5;
const PICK_INSET: f32 = 10.;
const PICK_RADIUS: f32 = 8.;
/// A borderless field sets its line above centre; measured, it sits 3.5pt above a centred label.
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

#[derive(Clone)]
enum GoLine {
    Heading(&'static str),
    Path(String),
}

#[derive(Clone)]
pub(crate) struct GoSheet {
    typed: String,
    lines: Vec<GoLine>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
}

impl GoSheet {
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
    let found = finder_fs::complete::complete(typed, Some(index()), CHOICES)
        .into_iter()
        .map(|found| found.path.to_string_lossy().into_owned());
    folder.into_iter().chain(found).collect()
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

fn ghost(sheet: &GoSheet) -> Option<String> {
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

/// A bare name lists matches in the current folder and among recents, as Finder does; a path, or
/// a name found in neither, lists completions.
fn go_lines(typed: &str, here: &str) -> Vec<GoLine> {
    let bare = !typed.contains('/') && !typed.starts_with('~');
    if !bare {
        return section("Go to:", go_rows(typed));
    }
    let nearby: Vec<String> = match here.starts_with('/') {
        true => {
            let inside = format!("{}/{typed}", here.trim_end_matches('/'));
            finder_fs::complete::complete(&inside, Some(index()), CHOICES)
                .into_iter()
                .map(|found| found.path.to_string_lossy().into_owned())
                .collect()
        }
        false => Vec::new(),
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

/// Finder draws each row as a path control, which clips the longest crumbs and keeps the leaf.
fn go_row(
    at: usize,
    path: &str,
    picked: bool,
    bold: Vec<usize>,
    go: WeakEntity<FinderDemo>,
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
                .w(px(PATH_W))
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

fn pick(go: &WeakEntity<FinderDemo>, at: usize, window: &mut Window, cx: &mut App) {
    go.update(cx, |demo: &mut FinderDemo, cx| {
        if let Some(sheet) = demo.go_sheet.as_mut() {
            sheet.selected = Some(at);
        }
        demo.go_submit("", window, cx);
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
    /// Finder opens on the current folder's path, selected, listing what is inside it.
    pub(crate) fn open_go(&mut self, cx: &mut Context<Self>) {
        self.go_sheet = Some(GoSheet {
            typed: String::new(),
            lines: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        });
        let here = self.here();
        let typed = match here.starts_with('/') {
            true => format!("{}/", here.trim_end_matches('/')),
            false => String::new(),
        };
        self.go_typing(typed, cx);
    }

    fn go_typing(&mut self, text: String, cx: &mut Context<Self>) {
        let here = self.here();
        let Some(sheet) = self.go_sheet.as_mut() else {
            return;
        };
        sheet.typed = text.clone();
        cx.notify();
        if text.is_empty() {
            sheet.lines = section("Recents", prefs::go_history());
            sheet.selected = None;
            return;
        }
        cx.spawn(async move |this, cx| {
            let wanted = text.clone();
            let lines = cx
                .background_executor()
                .spawn(async move { go_lines(&text, here) })
                .await;
            this.update(cx, |this, cx| {
                let Some(sheet) = this.go_sheet.as_mut() else {
                    return;
                };
                if sheet.typed != wanted {
                    return;
                }
                sheet.selected = lines
                    .iter()
                    .position(|line| matches!(line, GoLine::Path(_)));
                sheet.lines = lines;
                sheet.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Steps over the section headings.
    fn go_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(sheet) = self.go_sheet.as_mut() else {
            return;
        };
        let paths: Vec<usize> = (0..sheet.lines.len())
            .filter(|&at| matches!(sheet.lines[at], GoLine::Path(_)))
            .collect();
        let Some(last) = paths.len().checked_sub(1) else {
            return;
        };
        let now = sheet
            .selected
            .and_then(|selected| paths.iter().position(|&at| at == selected));
        let step = match now {
            None if by > 0 => 0,
            None => return,
            Some(step) => step.saturating_add_signed(by).min(last),
        };
        let at = paths[step];
        sheet.selected = Some(at);
        // The first path brings its heading into view with it.
        match step {
            0 => sheet.scroll.scroll_to_item(0, ScrollStrategy::Top),
            _ => sheet.scroll.scroll_to_item(at, ScrollStrategy::Nearest),
        }
        cx.notify();
    }

    fn go_accept(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.go_sheet.as_ref().and_then(GoSheet::chosen) else {
            return;
        };
        let text = format!("{}/", chosen.trim_end_matches('/'));
        self.go_typing(text, cx);
    }

    fn go_submit(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .go_sheet
            .as_ref()
            .and_then(GoSheet::chosen)
            .map_or_else(|| typed.to_string(), str::to_string);
        if target.trim().is_empty() {
            return;
        }
        self.go_typed(&target, window, cx);
    }

    fn close_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_sheet = None;
        focus_gpui(window);
        key_to_parent(window);
        cx.notify();
    }

    /// Off the main thread: a path on a volume that stopped answering must not hang the window.
    fn go_typed(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = typed.to_string();
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
            this.update_in(cx, |this, window, cx| {
                if refused {
                    return;
                }
                this.close_go(window, cx);
                match (folder, item) {
                    (Some(id), _) => this.go(id, window, cx),
                    (None, Some(item)) => this.show_enclosing(item, window, cx),
                    (None, None) => {}
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn go_sheet(
        &self,
        sheet: &GoSheet,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this: WeakEntity<Self> = cx.entity().downgrade();
        let (typing, commanded, submitting, leaving, clearing) = (
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
        );
        let field = native_text_field("ng.finder.go", &sheet.typed)
            .stretch()
            .borderless()
            .font_size(FIELD_FONT)
            .placeholder("Go to Folder")
            .editing(0..sheet.typed.chars().count())
            .on_change(move |text, _, cx| {
                let text = text.to_string();
                typing
                    .update(cx, |demo: &mut FinderDemo, cx| demo.go_typing(text, cx))
                    .ok();
            })
            .on_command(COMMANDS, move |command, text, _, cx| {
                commanded
                    .update(cx, |demo: &mut FinderDemo, cx| match command {
                        "moveUp:" => demo.go_move(-1, cx),
                        "moveDown:" => demo.go_move(1, cx),
                        "deleteWordBackward:" => demo.go_typing(drop_folder(text), cx),
                        _ => demo.go_accept(cx),
                    })
                    .ok();
            })
            .on_submit(move |typed, window, cx| {
                let typed = typed.to_string();
                submitting
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.go_submit(&typed, window, cx)
                    })
                    .ok();
            })
            .on_cancel(move |_, window, cx| {
                leaving
                    .update(cx, |demo: &mut FinderDemo, cx| demo.close_go(window, cx))
                    .ok();
            });

        let typed_w = text_width(window, sheet.typed.clone(), px(FIELD_FONT));
        let ghost = ghost(sheet).map(|ghost| {
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
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                clearing
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.go_typing(String::new(), cx)
                    })
                    .ok();
            });
        let entry = div()
            .absolute()
            .top(px(FIELD_Y + FIELD_DROP))
            .left(px(FIELD_X))
            .right(px(FIELD_X + CLEAR))
            .h(px(FIELD_H))
            .flex()
            .items_center()
            .overflow_hidden()
            .children(ghost)
            .child(field);

        let (lines, selected) = (sheet.lines.clone(), sheet.selected);
        let tail = sheet
            .typed
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        // Only the rows in view host controls.
        let list = uniform_list("ng.finder.go.rows", lines.len(), move |range, _, _| {
            range
                .map(|at| match &lines[at] {
                    GoLine::Heading(heading) => go_heading(at, heading),
                    GoLine::Path(path) => {
                        let bold = marks(&tail, leaf(path)).unwrap_or_default();
                        go_row(at, path, selected == Some(at), bold, this.clone())
                    }
                })
                .collect()
        })
        .size_full()
        .track_scroll(&sheet.scroll);

        let corner = system_corner_radius(window);
        let radius = f32::from(corner);
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
        let metrics = self.metrics();
        let width = f32::from(window.viewport_size().width);
        // Centred on the window as Finder's; the sidebar and pane are native views drawn over GPUI.
        let left = ((width - PANEL_W) / 2.)
            .min(width - f32::from(metrics.right) - PANEL_W)
            .max(f32::from(metrics.left));
        div()
            .absolute()
            .top(px(PANEL_TOP))
            .left(px(left))
            .child(
                div()
                    .relative()
                    .w(px(PANEL_W))
                    .h(px(PANEL_H))
                    .rounded(corner)
                    .shadow_lg()
                    .block_mouse_except_scroll()
                    // Opaque like Finder's, and native so it covers the list header's band. Gray
                    // fills the card, the field's white takes its top with the card's own corners,
                    // and a square gray strip hides the white's rounded bottom corners.
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
                    .child(band(SEPARATOR_Y, 1.).child(
                        native_rule("ng.finder.go.separator")
                            .tint(tokens::system_color(SystemColor::Separator)),
                    ))
                    .child(entry)
                    .child(clear)
                    .child(band(LIST_Y + LIST_PAD, PANEL_H - LIST_Y - LIST_PAD).child(list)),
            )
            .into_any_element()
    }

    fn connect(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        self.connect_sheet = Some(ConnectSheet {
            address: address.clone(),
            status: Some("Connecting…".into()),
        });
        cx.notify();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let wanted = address.clone();
            let mounted = cx
                .background_executor()
                .spawn(async move { finder_fs::volumes::mount(&wanted) })
                .await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| match mounted {
                    Ok(point) => {
                        this.connect_sheet = None;
                        focus_gpui(window);
                        this.go(intern_path(&point), window, cx);
                    }
                    Err(code) => {
                        let status = format!(
                            "There was a problem connecting to the server \u{201c}{address}\u{201d}. (Error {code})"
                        );
                        this.connect_sheet = Some(ConnectSheet {
                            address,
                            status: Some(status),
                        });
                        cx.notify();
                    }
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn connect_sheet(
        &self,
        sheet: &ConnectSheet,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this: WeakEntity<Self> = cx.entity().downgrade();
        let leave = this.clone();
        let field = native_text_field("ng.finder.connect", &sheet.address)
            .stretch()
            .placeholder("smb://server")
            .editing(0..sheet.address.chars().count())
            .on_submit(move |typed, window, cx| {
                let typed = typed.trim().to_string();
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    demo.connect(typed, window, cx)
                })
                .ok();
            })
            .on_cancel(move |_, window, cx| {
                leave
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.connect_sheet = None;
                        focus_gpui(window);
                        key_to_parent(window);
                        cx.notify();
                    })
                    .ok();
            });
        let status = sheet.status.as_deref().map(|status| {
            native_label("ng.finder.connect.status", status)
                .font_size(px(12.))
                .color(SystemColor::SecondaryLabel)
                .into_any_element()
        });
        glass(
            "ng.finder.connect.glass",
            "Connect to Server",
            field,
            status,
            self.metrics().top,
            window,
        )
    }

    pub(crate) fn save_sheet(
        &self,
        name: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this: WeakEntity<Self> = cx.entity().downgrade();
        let leave = this.clone();
        let field = native_text_field("ng.finder.save", name)
            .stretch()
            .editing(0..name.chars().count())
            .on_submit(move |typed, window, cx| {
                let typed = typed.trim().to_string();
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    demo.save_sheet = None;
                    focus_gpui(window);
                    key_to_parent(window);
                    if !typed.is_empty() {
                        demo.save_search(&typed, window, cx);
                    }
                    cx.notify();
                })
                .ok();
            })
            .on_cancel(move |_, window, cx| {
                leave
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.save_sheet = None;
                        focus_gpui(window);
                        key_to_parent(window);
                        cx.notify();
                    })
                    .ok();
            });
        glass(
            "ng.finder.save.glass",
            "Save As:",
            field,
            None,
            self.metrics().top,
            window,
        )
    }
}

#[derive(Clone, Default)]
pub(crate) struct ConnectSheet {
    address: String,
    status: Option<String>,
}

fn glass(
    id: &'static str,
    title: &'static str,
    field: impl IntoElement,
    below: Option<AnyElement>,
    top: Pixels,
    window: &Window,
) -> AnyElement {
    let corner = system_corner_radius(window);
    div()
        .absolute()
        .top(top)
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
                    material(id, Material::Popover)
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
                            native_label((id, 1usize), title)
                                .font_size(px(13.))
                                .weight(FontWeight::SEMIBOLD),
                        )
                        .child(div().h(px(24.)).child(field))
                        .children(below),
                ),
        )
        .into_any_element()
}
