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

fn field(control: AnyElement, readout: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .child(sized(FIELD_WIDTH, control))
        .child(mono(readout))
        .into_any_element()
}

fn submit(d: &mut Demo, value: String) {
    d.text.submitted = Some(value);
    d.text.submits += 1;
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let State {
        text,
        secure,
        search,
        bare,
        submitted,
        submits,
    } = &d.text;

    stack(vec![
        demo(
            "plain — mirrored into GPUI on every keystroke",
            field(
                native_text_field("plain", text)
                    .placeholder("Project name")
                    .on_change(set(cx, |d, value| d.text.text = value))
                    .on_submit(set(cx, submit))
                    .into_any_element(),
                format!("{text:?}"),
            ),
        ),
        demo(
            "on_submit — press Return in the field above",
            match submitted {
                Some(value) => mono(format!("{value:?} · {submits} submit(s)")),
                None => caption("nothing submitted yet"),
            },
        ),
        demo(
            "secure — bullets, and the system's secure input path",
            field(
                native_secure_field("secure", secure)
                    .placeholder("Password")
                    .on_change(set(cx, |d, value| d.text.secure = value))
                    .into_any_element(),
                format!("{} character(s)", secure.chars().count()),
            ),
        ),
        demo(
            "search — magnifier and cancel button included",
            field(
                native_search_field("search", search)
                    .placeholder("Filter")
                    .on_change(set(cx, |d, value| d.text.search = value))
                    .on_submit(set(cx, submit))
                    .into_any_element(),
                format!("{search:?}"),
            ),
        ),
        section(
            "no placeholder, and disabled",
            vec![
                demo(
                    "placeholder omitted",
                    sized(
                        FIELD_WIDTH,
                        native_text_field("bare", bare)
                            .on_change(set(cx, |d, value| d.text.bare = value))
                            .into_any_element(),
                    ),
                ),
                demo(
                    "enabled(false)",
                    sized(
                        FIELD_WIDTH,
                        native_text_field("disabled", text)
                            .placeholder("Project name")
                            .enabled(false)
                            .into_any_element(),
                    ),
                ),
                demo(
                    "enabled(false), search",
                    sized(
                        FIELD_WIDTH,
                        native_search_field("disabled-search", search)
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
