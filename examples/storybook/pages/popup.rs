//! `NSPopUpButton` in both styles, and a real `NSMenu` hung off a GPUI-drawn region.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Modifiers, SharedString, Window, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{MenuEntry, MenuOrigin, native_context_menu, native_popup};

#[derive(Default)]
pub struct State {
    pub colour: usize,
    pub tool: usize,
    pub action: Option<String>,
    pub picked: Option<String>,
}

const COLOURS: &[&str] = &["Graphite", "Blue", "Purple", "Green"];
const EDITS: &[&str] = &["Cut", "Copy", "Paste"];

fn tools() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Select").symbol("cursorarrow"),
        MenuEntry::item("Draw").symbol("pencil"),
        MenuEntry::separator(),
        MenuEntry::item("Rotate").symbol("arrow.clockwise"),
        MenuEntry::item("Crop").symbol("crop").disabled(),
    ]
}

fn actions() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Actions"),
        MenuEntry::item("Duplicate").symbol("doc.on.doc"),
        MenuEntry::item("Export…").symbol("square.and.arrow.up"),
        MenuEntry::separator(),
        MenuEntry::item("Delete").symbol("trash").disabled(),
    ]
}

fn row_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Open").symbol("arrow.up.forward.app"),
        MenuEntry::item("Rename").symbol("pencil"),
        MenuEntry::separator(),
        MenuEntry::item("Move to Trash").symbol("trash"),
        MenuEntry::item("Get Info").symbol("info.circle").disabled(),
    ]
}

/// The rest of `MenuEntry`: a checkmark, a chord label and a nested submenu.
fn full_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::item("Show Hidden Files").checked(true),
        MenuEntry::item("Show Path Bar").checked(false),
        MenuEntry::separator(),
        MenuEntry::item("Open").key("o", Modifiers::command()),
        MenuEntry::item("Open in New Window").key("o", Modifiers::command_shift()),
        MenuEntry::separator(),
        MenuEntry::item("Sort By")
            .symbol("arrow.up.arrow.down")
            .submenu([
                MenuEntry::item("Name"),
                MenuEntry::item("Date Modified"),
                MenuEntry::item("Size"),
            ]),
    ]
}

/// Every index this module reports indexes the entry list directly, separators included.
fn title_of(entries: &[MenuEntry], index: usize) -> String {
    match entries.get(index) {
        Some(MenuEntry::Item { title, .. }) => title.clone(),
        Some(MenuEntry::Separator) => "(separator)".into(),
        Some(MenuEntry::View(_)) => "(custom view)".into(),
        None => format!("(out of range: {index})"),
    }
}

fn name_of(names: &[&str], index: usize) -> String {
    names.get(index).map_or_else(
        || format!("(out of range: {index})"),
        |name| name.to_string(),
    )
}

fn target(label: &str) -> AnyElement {
    div()
        .w(px(240.))
        .h(px(88.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(system_color(SystemColor::Separator))
        .bg(system_color(SystemColor::TertiarySystemFill))
        .text_sm()
        .text_color(system_color(SystemColor::SecondaryLabel))
        .child(SharedString::from(label.to_string()))
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (colour, tool) = (d.popup.colour, d.popup.tool);
    let action = d.popup.action.clone();
    let picked = d.popup.picked.clone();

    stack(vec![
        row(
            "pop up",
            vec![
                native_popup("colour", colour)
                    .items(COLOURS.iter().copied())
                    .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                        d.popup.colour = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
                mono(format!("{colour} — {}", name_of(COLOURS, colour))),
                native_popup("colour-off", colour)
                    .items(COLOURS.iter().copied())
                    .enabled(false)
                    .into_any_element(),
            ],
        ),
        row(
            "entries — SF Symbols, a separator, a disabled row",
            vec![
                native_popup("tool", tool)
                    .entries(tools())
                    .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                        d.popup.tool = *index;
                        cx.notify();
                    }))
                    .into_any_element(),
                mono(format!("{tool} — {}", title_of(&tools(), tool))),
            ],
        ),
        row(
            "pull down",
            vec![
                native_popup("actions", 0)
                    .entries(actions())
                    .pull_down(true)
                    .on_change(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                        d.popup.action =
                            Some(format!("{index} — {}", title_of(&actions(), *index)));
                        cx.notify();
                    }))
                    .into_any_element(),
                mono(action.clone().unwrap_or_else(|| "nothing picked".into())),
            ],
        ),
        note(
            "A pull-down lifts entry 0 onto the button face and hides it from the list, so its \
             pickable rows start at index 1. Crop and Delete are disabled entries, and a \
             separator holds its own index in both styles.",
        ),
        section(
            "context menu",
            vec![
                demo(
                    "right-click or control-click — menu at the pointer",
                    native_context_menu("ctx-pointer", target("Right-click anywhere in here"))
                        .entries(row_entries())
                        .on_select(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                            d.popup.picked =
                                Some(format!("{index} — {}", title_of(&row_entries(), *index)));
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                demo(
                    "items(..) instead of entries(..), opening at the element's top-left",
                    native_context_menu(
                        "ctx-corner",
                        target("Right-click: menu opens at the corner"),
                    )
                    .items(EDITS.iter().copied())
                    .origin(MenuOrigin::ElementTopLeft)
                    .on_select(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                        d.popup.picked = Some(format!("{index} — {}", name_of(EDITS, *index)));
                        cx.notify();
                    }))
                    .into_any_element(),
                ),
                caption("last context-menu pick"),
                mono(picked.clone().unwrap_or_else(|| "nothing picked".into())),
            ],
        ),
        section(
            "the rest of MenuEntry",
            vec![
                demo(
                    "right-click — checkmarks, chord labels and a submenu",
                    native_context_menu("ctx-full", target("Right-click for the full set"))
                        .entries(full_entries())
                        .on_select(cx.listener(|d: &mut Demo, index: &usize, _, cx| {
                            d.popup.picked =
                                Some(format!("{index} — {}", title_of(&full_entries(), *index)));
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "checked(bool) draws macOS's own checkmark in the leading gutter every row \
                     already reserves, so a menu with one ticked row does not indent the others. \
                     key(equivalent, modifiers) is a label only outside the menu bar — a \
                     transient menu is up only while it is tracking and nothing is listening for \
                     the chord. Pass the unmodified lowercase character; AppKit draws the glyph \
                     and adds ⇧ itself.",
                ),
                caption(
                    "submenu(entries) hangs a real NSMenu off the row, and AppKit draws the ›, \
                     opens it on hover and sizes it. Indices stay flat, so a submenu's rows are \
                     unreachable by design: they carry no action and choosing one reports \
                     nothing, rather than a number that would mean a different row of the outer \
                     list. The host row cannot report either — setSubmenu: installs AppKit's own \
                     submenuAction:. Pick Sort By › Name above and the line below does not move.",
                ),
                caption(
                    "MenuEntry::view(MenuView) is the fourth kind and is not shown here: it is a \
                     retained NSView of the caller's own, outside GPUI layout and pooling, and \
                     like a submenu row it is never reported.",
                ),
            ],
        ),
        props(&[
            ("native_popup(id, selected)", format!("{colour} / {tool}")),
            ("items(iter)", "plain rows, all enabled".into()),
            (
                "entries(iter)",
                "MenuEntry::item / ::separator / ::view".into(),
            ),
            (
                "MenuEntry builders",
                "symbol · disabled · checked · key · submenu".into(),
            ),
            (
                "pull_down(bool)",
                action.unwrap_or_else(|| "pickable rows start at 1".into()),
            ),
            ("enabled(bool)", "false on the second pop-up".into()),
            (
                "native_context_menu(id, child)",
                picked.unwrap_or_else(|| "right-click a target above".into()),
            ),
            (
                "origin(MenuOrigin)",
                "Pointer (default) / ElementTopLeft".into(),
            ),
            (
                "index source",
                "indexOfSelectedItem, or the picked item's tag".into(),
            ),
        ]),
    ])
}
