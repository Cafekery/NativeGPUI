use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*};
use native_gpui::{native_search_field, native_secure_field, native_text_field};

pub struct State {
    pub text: String,
    pub secure: String,
    pub search: String,
    pub bare: String,
    pub submitted: Option<String>,
    pub submits: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            text: "Storybook".into(),
            secure: String::new(),
            search: String::new(),
            bare: String::new(),
            submitted: None,
            submits: 0,
        }
    }
}

const FIELD_WIDTH: f32 = 280.;

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let text = d.text.text.clone();
    let secure = d.text.secure.clone();
    let search = d.text.search.clone();
    let bare = d.text.bare.clone();
    let submitted = d.text.submitted.clone();
    let submits = d.text.submits;

    stack(vec![
        demo(
            "plain — mirrored into GPUI on every keystroke",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(sized(
                    FIELD_WIDTH,
                    native_text_field("plain", &text)
                        .placeholder("Project name")
                        .on_change(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                            d.text.text = value.clone();
                            cx.notify();
                        }))
                        .on_submit(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                            d.text.submitted = Some(value.clone());
                            d.text.submits += 1;
                            cx.notify();
                        }))
                        .into_any_element(),
                ))
                .child(mono(format!("{text:?}")))
                .into_any_element(),
        ),
        demo(
            "on_submit — press Return in the field above",
            match &submitted {
                Some(value) => mono(format!("{value:?} · {submits} submit(s)")),
                None => caption("nothing submitted yet"),
            },
        ),
        demo(
            "secure — bullets, and the system's secure input path",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(sized(
                    FIELD_WIDTH,
                    native_secure_field("secure", &secure)
                        .placeholder("Password")
                        .on_change(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                            d.text.secure = value.clone();
                            cx.notify();
                        }))
                        .into_any_element(),
                ))
                .child(mono(format!("{} character(s)", secure.chars().count())))
                .into_any_element(),
        ),
        demo(
            "search — magnifier and cancel button included",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(sized(
                    FIELD_WIDTH,
                    native_search_field("search", &search)
                        .placeholder("Filter")
                        .on_change(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                            d.text.search = value.clone();
                            cx.notify();
                        }))
                        .on_submit(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                            d.text.submitted = Some(value.clone());
                            d.text.submits += 1;
                            cx.notify();
                        }))
                        .into_any_element(),
                ))
                .child(mono(format!("{search:?}")))
                .into_any_element(),
        ),
        section(
            "no placeholder, and disabled",
            vec![
                demo(
                    "placeholder omitted",
                    sized(
                        FIELD_WIDTH,
                        native_text_field("bare", &bare)
                            .on_change(cx.listener(|d: &mut Demo, value: &String, _, cx| {
                                d.text.bare = value.clone();
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                ),
                demo(
                    "enabled(false)",
                    sized(
                        FIELD_WIDTH,
                        native_text_field("disabled", &text)
                            .placeholder("Project name")
                            .enabled(false)
                            .into_any_element(),
                    ),
                ),
                demo(
                    "enabled(false), search",
                    sized(
                        FIELD_WIDTH,
                        native_search_field("disabled-search", &search)
                            .placeholder("Filter")
                            .enabled(false)
                            .into_any_element(),
                    ),
                ),
            ],
        ),
        note(
            "Type here and the field is the real thing: IME, spell check, emoji, Writing Tools, \
             AutoFill and the system focus ring all arrive with the view, not from this crate.",
        ),
        props(&[
            ("native_text_field(id, &str)", format!("{text:?}")),
            ("native_secure_field(id, &str)", "bullets".into()),
            ("native_search_field(id, &str)", format!("{search:?}")),
            (
                "placeholder(impl Into<SharedString>)",
                "set on three".into(),
            ),
            ("enabled(bool)", "false on the last two".into()),
            ("on_change(Fn(&String, ..))", "every keystroke".into()),
            (
                "on_submit(Fn(&String, ..))",
                format!("Return · {submits} so far"),
            ),
            (
                "⌘A / ⌘C / ⌘V / ⌘X / ⌘Z",
                "reach the field because install_menus ran — macOS takes those key equivalents \
                 from the main menu, and a GPUI app installs none."
                    .into(),
            ),
            (
                "arrows, forward-delete",
                "need no opt-in: the host adopts the window's key path when it places a view."
                    .into(),
            ),
            (
                "Home / End",
                "do not move the caret, here or in an unhosted field — the editor scrolls instead."
                    .into(),
            ),
        ]),
    ])
}
