use finder_fs::sort::fold;
use gpui::{AnyElement, Context, FontWeight, WeakEntity, Window, div, prelude::*, px};
use native_gpui::tokens;
use native_gpui::{Material, material, system_corner_radius};

use crate::app::FinderDemo;
use crate::data::intern_path;
use crate::navigate::{folder_at_path, typed_path};
use crate::palette::Palette;
use crate::ui::{focus_gpui, key_to_parent, native_text_field};
use crate::{home, index};

const SHEET_W: f32 = 520.;
const CHOICES: usize = 8;

#[derive(Clone)]
pub(crate) struct GoSheet {
    typed: String,
    shown: String,
    choices: Vec<String>,
}

impl GoSheet {
    pub(crate) fn at(path: String) -> GoSheet {
        GoSheet {
            shown: path.clone(),
            typed: path,
            choices: Vec::new(),
        }
    }
}

fn as_typed(typed: &str, display: String) -> String {
    match display.strip_prefix('~') {
        Some(rest) if !typed.starts_with('~') => format!("{}{rest}", home().to_string_lossy()),
        _ => display,
    }
}

fn inline(typed: &str, choices: &[String]) -> Option<String> {
    let best = choices.first()?;
    let (head, tail) = best.split_at_checked(typed.len())?;
    (!typed.ends_with('/') && !tail.is_empty() && fold(head) == fold(typed)).then(|| best.clone())
}

impl FinderDemo {
    /// The completion sits selected after the typing: typing replaces it and a delete removes it.
    fn go_typing(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(sheet) = self.go_sheet.as_ref() else {
            return;
        };
        let typed = match text.strip_prefix(sheet.shown.as_str()) {
            Some(more) if !more.is_empty() => format!("{}{more}", sheet.typed),
            _ if sheet.shown.starts_with(&text) => match text.len() < sheet.typed.len() {
                true => text,
                false => sheet.typed.clone(),
            },
            _ => text,
        };
        let completing = typed.len() > sheet.typed.len();
        self.go_sheet = Some(GoSheet::at(typed.clone()));
        cx.notify();
        if !completing {
            return;
        }
        let wanted = typed.clone();
        cx.spawn(async move |this, cx| {
            let choices = cx
                .background_executor()
                .spawn(async move {
                    finder_fs::complete::complete(&typed, Some(index()), CHOICES)
                        .into_iter()
                        .map(|found| as_typed(&typed, found.display))
                        .collect::<Vec<String>>()
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(sheet) = this.go_sheet.as_mut() else {
                    return;
                };
                if sheet.typed != wanted {
                    return;
                }
                sheet.shown = inline(&sheet.typed, &choices).unwrap_or_else(|| sheet.typed.clone());
                sheet.choices = choices;
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let leave = this.clone();
        let typing = this.clone();
        let field = native_text_field("ng.finder.go", &sheet.shown)
            .stretch()
            .editing(0..sheet.shown.chars().count())
            .on_change(move |text, _, cx| {
                let text = text.to_string();
                typing
                    .update(cx, |demo: &mut FinderDemo, cx| demo.go_typing(text, cx))
                    .ok();
            })
            .on_submit(move |typed, window, cx| {
                let typed = typed.to_string();
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    demo.go_typed(&typed, window, cx);
                    demo.go_sheet = None;
                    focus_gpui(window);
                    key_to_parent(window);
                    cx.notify();
                })
                .ok();
            })
            .on_cancel(move |_, window, cx| {
                leave
                    .update(cx, |demo: &mut FinderDemo, cx| {
                        demo.go_sheet = None;
                        focus_gpui(window);
                        key_to_parent(window);
                        cx.notify();
                    })
                    .ok();
            });

        let choices = (!sheet.choices.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(sheet.choices.iter().enumerate().map(|(at, choice)| {
                    div()
                        .text_size(px(12.))
                        .text_color(match at {
                            0 => palette.label,
                            _ => palette.secondary,
                        })
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(choice.clone())
                }))
                .into_any_element()
        });
        glass(
            "ng.finder.go.glass",
            "Go to the folder:",
            field,
            choices,
            palette,
            window,
        )
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
