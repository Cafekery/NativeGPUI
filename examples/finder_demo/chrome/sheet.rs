use finder_fs::sort::fold;
use gpui::{
    AnyElement, Context, FontWeight, MouseButton, ScrollHandle, WeakEntity, Window, div,
    prelude::*, px,
};
use native_gpui::tokens::{self, SymbolColor, SystemColor, sf_symbol_colored};
use native_gpui::{Material, material, system_corner_radius};

use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::navigate::{folder_at_path, typed_path};
use crate::palette::Palette;
use crate::prefs;
use crate::ui::{focus_gpui, key_to_parent, native_text_field, text_width};
use crate::{home, index};

const SHEET_W: f32 = 520.;
const PANEL_W: f32 = 460.;
const PANEL_TOP: f32 = 60.;
const ROW_H: f32 = 22.;
const ROWS_SHOWN: f32 = 7.;
const TEXT: f32 = 13.;
const FIELD_TEXT_INSET: f32 = 2.;
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
pub(crate) struct GoSheet {
    typed: String,
    rows: Vec<String>,
    selected: Option<usize>,
    scroll: ScrollHandle,
}

impl GoSheet {
    pub(crate) fn open() -> GoSheet {
        GoSheet {
            typed: String::new(),
            rows: prefs::go_history(),
            selected: None,
            scroll: ScrollHandle::new(),
        }
    }

    fn chosen(&self) -> Option<&str> {
        self.rows.get(self.selected?).map(String::as_str)
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

fn remember_go(path: &str) {
    let mut history = prefs::go_history();
    history.retain(|seen| seen != path);
    history.insert(0, path.to_string());
    history.truncate(HISTORY);
    prefs::set_go_history(&history.iter().map(String::as_str).collect::<Vec<_>>());
}

impl FinderDemo {
    fn go_typing(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(sheet) = self.go_sheet.as_mut() else {
            return;
        };
        sheet.typed = text.clone();
        cx.notify();
        if text.is_empty() {
            sheet.rows = prefs::go_history();
            sheet.selected = None;
            return;
        }
        cx.spawn(async move |this, cx| {
            let wanted = text.clone();
            let rows = cx
                .background_executor()
                .spawn(async move { go_rows(&text) })
                .await;
            this.update(cx, |this, cx| {
                let Some(sheet) = this.go_sheet.as_mut() else {
                    return;
                };
                if sheet.typed != wanted {
                    return;
                }
                sheet.selected = (!rows.is_empty()).then_some(0);
                sheet.rows = rows;
                sheet.scroll.scroll_to_item(0);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn go_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(sheet) = self.go_sheet.as_mut() else {
            return;
        };
        let Some(last) = sheet.rows.len().checked_sub(1) else {
            return;
        };
        let at = match sheet.selected {
            None if by > 0 => 0,
            None => return,
            Some(at) => at.saturating_add_signed(by).min(last),
        };
        sheet.selected = Some(at);
        sheet.scroll.scroll_to_item(at);
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
        self.close_go(window, cx);
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
            let (folder, item) = cx
                .background_executor()
                .spawn(async move {
                    let folder = folder_at_path(&typed);
                    let path = typed_path(&typed);
                    let item = (folder.is_none() && path.symlink_metadata().is_ok())
                        .then(|| intern_path(&path));
                    if folder.is_some() || item.is_some() {
                        remember_go(&path.to_string_lossy());
                    }
                    (folder, item)
                })
                .await;
            this.update_in(cx, |this, window, cx| match (folder, item) {
                (Some(id), _) => this.go(id, window, cx),
                (None, Some(item)) => this.show_enclosing(item, window, cx),
                (None, None) => {}
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn go_sheet(
        &self,
        sheet: &GoSheet,
        palette: &Palette,
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

        let typed_w = text_width(window, sheet.typed.clone(), px(TEXT));
        let ghost = ghost(sheet).map(|ghost| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(FIELD_TEXT_INSET + typed_w))
                .flex()
                .items_center()
                .text_size(px(TEXT))
                .text_color(palette.tertiary)
                .whitespace_nowrap()
                .child(ghost)
        });
        let clear = div()
            .id("ng.finder.go.clear")
            .flex_none()
            .children(
                sf_symbol_colored(
                    "xmark.circle.fill",
                    px(TEXT),
                    FontWeight::NORMAL,
                    &SymbolColor::Hierarchical(SystemColor::TertiaryLabel),
                )
                .map(|symbol| symbol.img()),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                clearing
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.go_typing(String::new(), cx)
                    })
                    .ok();
            });
        let entry = div()
            .h(px(24.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .children(ghost)
                    .child(field),
            )
            .child(clear);

        let bold_leaf = !sheet.typed.is_empty() && !sheet.typed.contains('/');
        let rows = sheet.rows.iter().enumerate().map(|(at, path)| {
            let picked = sheet.selected == Some(at);
            let (text, crumb) = if picked {
                (palette.selection_text, palette.selection_text.opacity(0.7))
            } else {
                (palette.label, palette.secondary)
            };
            let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
            let last = parts.len().saturating_sub(1);
            let crumbs = parts.iter().enumerate().flat_map(|(index, part)| {
                let name = div()
                    .when(bold_leaf && index == last, |name| {
                        name.font_weight(FontWeight::SEMIBOLD)
                    })
                    .text_color(text)
                    .child(part.to_string());
                let arrow = (index < last).then(|| div().text_color(crumb).child("\u{203a}"));
                std::iter::once(name).chain(arrow)
            });
            let go = this.clone();
            div()
                .id(("ng.finder.go.row", at))
                .h(px(ROW_H))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .rounded(px(6.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(TEXT))
                .when(picked, |row| row.bg(palette.selection))
                .when(parts.is_empty(), |row| row.text_color(text).child("/"))
                .children(crumbs)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    go.update(cx, |demo: &mut FinderDemo, cx| {
                        if let Some(sheet) = demo.go_sheet.as_mut() {
                            sheet.selected = Some(at);
                        }
                        demo.go_submit("", window, cx);
                    })
                    .ok();
                })
        });
        let list = (!sheet.rows.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .px(px(8.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.secondary)
                        .child(if sheet.typed.is_empty() {
                            "Recents"
                        } else {
                            "Go to:"
                        }),
                )
                .child(
                    div()
                        .id("ng.finder.go.rows")
                        .max_h(px(ROW_H * ROWS_SHOWN))
                        .overflow_y_scroll()
                        .track_scroll(&sheet.scroll)
                        .flex()
                        .flex_col()
                        .children(rows),
                )
        });

        div()
            .absolute()
            .top(px(PANEL_TOP))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .relative()
                    .w(px(PANEL_W))
                    .rounded(system_corner_radius(window))
                    .overflow_hidden()
                    .shadow_lg()
                    .bg(tokens::system_color(SystemColor::WindowBackground))
                    .border_1()
                    .border_color(palette.separator)
                    .child(
                        div()
                            .relative()
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .font_family(tokens::system_font_family())
                            .text_color(palette.label)
                            .child(div().px(px(6.)).child(entry))
                            .children(list),
                    ),
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
        palette: &Palette,
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
        let status = sheet.status.clone().map(|status| {
            div()
                .text_size(px(12.))
                .text_color(palette.secondary)
                .child(status)
                .into_any_element()
        });
        glass(
            "ng.finder.connect.glass",
            "Connect to Server",
            field,
            status,
            palette,
            window,
        )
    }

    pub(crate) fn save_sheet(
        &self,
        name: &str,
        palette: &Palette,
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
            palette,
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
    palette: &Palette,
    window: &Window,
) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .relative()
                .w(px(SHEET_W))
                .rounded_b(system_corner_radius(window))
                .overflow_hidden()
                .child(material(id, Material::Popover).absolute().inset_0())
                .child(
                    div()
                        .relative()
                        .px(px(20.))
                        .py(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .font_family(tokens::system_font_family())
                        .text_color(palette.label)
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().h(px(24.)).child(field))
                        .children(below),
                ),
        )
        .into_any_element()
}
