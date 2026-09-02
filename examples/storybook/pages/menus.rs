use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, App, Context, SharedString, WeakEntity, Window, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    MenuCommand, MenuSection, TrackingMode, add_menu, extend_menu, install_menus, native_segmented,
};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenuItem};
use objc2_foundation::MainThreadMarker;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_storybook)]
struct MenuPing;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_storybook)]
struct MenuExport;

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = ng_storybook)]
struct MenuSettings;

#[derive(Default)]
pub struct State {
    pub listening: bool,
    pub added: bool,
    pub file_row: bool,
    pub app_row: bool,
    pub ping: usize,
    pub export: usize,
    pub settings: usize,
    pub reinstalls: usize,
}

struct BarRow {
    title: String,
    chord: String,
    selector: String,
    separator: bool,
    alternate: bool,
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    if !d.menus.listening {
        d.menus.listening = true;
        let entity = cx.entity().downgrade();
        listen(entity, cx);
    }

    let bar = read_bar();
    let rows: usize = bar.iter().map(|(_, rows)| rows.len()).sum();
    let (added, file_row, app_row) = (d.menus.added, d.menus.file_row, d.menus.app_row);
    let fired = (d.menus.ping, d.menus.export, d.menus.settings);

    stack(vec![
        note(
            "Nothing on this page is drawn by the crate. The bar at the top of the screen is the \
             one install_menus built, and the table below is that same NSMenu read back — titles, \
             key equivalents and the selector each row sends.",
        ),
        section(
            &format!("The live bar — {} menus, {rows} rows", bar.len()),
            vec![
                caption(
                    "Rows AppKit hides behind ⌥ are listed too, marked alt, so Close All appears \
                     here and not in the menu. macOS also inserts rows of its own the first time a \
                     menu is opened — Writing Tools, AutoFill, Start Dictation, Emoji & Symbols — \
                     so open the Edit menu and re-read to watch the count grow. Inheriting all of \
                     that is the point of using the real selectors.",
                ),
                grid(
                    bar.iter()
                        .map(|(title, rows)| menu_card(title, rows))
                        .collect(),
                ),
                demo(
                    "re-read",
                    native_segmented("reread", 0)
                        .segments(["Re-read NSApp.mainMenu"])
                        .tracking_mode(TrackingMode::Momentary)
                        .on_change(cx.listener(|_: &mut Demo, _: &usize, _, cx| {
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
            ],
        ),
        section(
            "add_menu / extend_menu",
            vec![
                caption(
                    "add_menu appends a whole menu of the app's own between View and Window, \
                     where Finder keeps Go. extend_menu puts rows inside a standard menu and \
                     keeps the rest of it standard — in the App menu they land in the reserved \
                     block below About, because appending would put them under Quit.",
                ),
                row(
                    "add rows to the bar above",
                    vec![
                        native_segmented("add-menu", 0)
                            .segments([if added {
                                "Storybook menu added"
                            } else {
                                "add_menu(\"Storybook\", …)"
                            }])
                            .tracking_mode(TrackingMode::Momentary)
                            .enabled(!added)
                            .on_change(cx.listener(|d: &mut Demo, _: &usize, _, cx| {
                                add_menu(
                                    "Storybook",
                                    [
                                        MenuCommand::new("Ping", MenuPing).key("cmd-ctrl-alt-p"),
                                        MenuCommand::separator(),
                                        MenuCommand::new("Ping, no chord", MenuPing),
                                    ],
                                );
                                d.menus.added = true;
                                cx.notify();
                            }))
                            .into_any_element(),
                        native_segmented("add-file", 0)
                            .segments([if file_row {
                                "File row added"
                            } else {
                                "extend_menu(File, …)"
                            }])
                            .tracking_mode(TrackingMode::Momentary)
                            .enabled(!file_row)
                            .on_change(cx.listener(|d: &mut Demo, _: &usize, _, cx| {
                                extend_menu(
                                    MenuSection::File,
                                    [
                                        MenuCommand::new("Export…", MenuExport)
                                            .key("cmd-ctrl-alt-e"),
                                    ],
                                );
                                d.menus.file_row = true;
                                cx.notify();
                            }))
                            .into_any_element(),
                        native_segmented("add-app", 0)
                            .segments([if app_row {
                                "App row added"
                            } else {
                                "extend_menu(App, …)"
                            }])
                            .tracking_mode(TrackingMode::Momentary)
                            .enabled(!app_row)
                            .on_change(cx.listener(|d: &mut Demo, _: &usize, _, cx| {
                                extend_menu(
                                    MenuSection::App,
                                    [MenuCommand::new("Storybook Settings…", MenuSettings)
                                        .key("cmd-,")],
                                );
                                d.menus.app_row = true;
                                cx.notify();
                            }))
                            .into_any_element(),
                    ],
                ),
                caption(format!(
                    "rows fired: Ping {} · Export {} · Settings {}. Pick one from the bar above — \
                     the count is the evidence, because a row that looks right and dispatches \
                     nothing looks identical.",
                    fired.0, fired.1, fired.2
                )),
            ],
        ),
        section(
            "A second install_menus is a no-op",
            vec![
                caption(
                    "Not a nicety. install_menus recognises its own bar by the bar's title, which \
                     nothing displays and nothing else sets. Recognising it by the first menu's \
                     title being the app's name — which is what this used to do — matches the bar \
                     a bundled app already has at startup, so the call returns having installed \
                     nothing and the app comes up with no ⌘W, ⌘Q, ⌘M or ⌘H at all. That failed \
                     only when bundled, which is the configuration every shipped app is in.",
                ),
                row(
                    "call it again",
                    vec![
                        native_segmented("reinstall", 0)
                            .segments(["install_menus(cx)"])
                            .tracking_mode(TrackingMode::Momentary)
                            .on_change(cx.listener(|d: &mut Demo, _: &usize, _, cx| {
                                install_menus(cx);
                                d.menus.reinstalls += 1;
                                cx.notify();
                            }))
                            .into_any_element(),
                        mono(format!("{} extra calls", d.menus.reinstalls)),
                    ],
                ),
                caption(
                    "Anything added above has to still be in the table after the call. If the \
                     guard were missing the bar would be rebuilt and an app's own menus silently \
                     wiped, while the standard rows looked perfect.",
                ),
            ],
        ),
        section(
            "Which layer answers a chord",
            vec![
                caption(
                    "AppKit offers a ⌘-chord to the key window's views first and to the menu bar \
                     only after the window declines, so GPUI's keymap wins every chord it binds \
                     and the bar never sees that one. Binding ⌘W in your keymap is what stops \
                     Close closing. The standard set therefore binds nothing into GPUI's keymap.",
                ),
                caption(
                    "The order for one chord in a window running this crate: hosted AppKit \
                     subviews first, then GPUI's keymap while GPUI's view is first responder, \
                     then this bar. Nothing on this page can demonstrate that — a chord typed \
                     here is a chord the storybook itself answers. scripts/keys.sh drives real \
                     System Events keystrokes for it, and scripts/menubar.sh reads the bar back \
                     out of a second process so the probe cannot agree with itself.",
                ),
            ],
        ),
        props(&[
            (
                "install_menus(cx)",
                format!(
                    "called once in main; {} further calls, all no-ops",
                    d.menus.reinstalls
                ),
            ),
            (
                "add_menu(title, [MenuCommand])",
                if added {
                    "Storybook, inserted before Window".into()
                } else {
                    "not added yet".into()
                },
            ),
            (
                "extend_menu(MenuSection, [MenuCommand])",
                format!(
                    "File {} · App {}",
                    if file_row { "extended" } else { "—" },
                    if app_row { "extended" } else { "—" }
                ),
            ),
            (
                "MenuCommand::new(title, Action)",
                "dispatched on the next run-loop turn, not in place".into(),
            ),
            (
                "MenuCommand::key(chord)",
                "spelled as KeyBinding spells one; a typo is a row with no equivalent".into(),
            ),
            ("MenuCommand::separator()", "one row, as in NSMenu".into()),
            ("MenuSection", "App, File, Edit, View, Window, Help".into()),
            (
                "rows fired",
                format!(
                    "Ping {} · Export {} · Settings {}",
                    fired.0, fired.1, fired.2
                ),
            ),
            (
                "NSApp.mainMenu",
                format!("{} menus, {rows} rows", bar.len()),
            ),
            (
                "App::set_menus",
                "never call it after install_menus — it replaces the whole bar".into(),
            ),
        ]),
    ])
}

fn listen(entity: WeakEntity<Demo>, cx: &mut App) {
    let ping = entity.clone();
    cx.on_action(move |_: &MenuPing, cx: &mut App| {
        ping.update(cx, |d: &mut Demo, cx| {
            d.menus.ping += 1;
            cx.notify();
        })
        .ok();
    });
    let export = entity.clone();
    cx.on_action(move |_: &MenuExport, cx: &mut App| {
        export
            .update(cx, |d: &mut Demo, cx| {
                d.menus.export += 1;
                cx.notify();
            })
            .ok();
    });
    cx.on_action(move |_: &MenuSettings, cx: &mut App| {
        entity
            .update(cx, |d: &mut Demo, cx| {
                d.menus.settings += 1;
                cx.notify();
            })
            .ok();
    });
}

fn read_bar() -> Vec<(String, Vec<BarRow>)> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let Some(bar) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return Vec::new();
    };
    bar.itemArray()
        .iter()
        .map(|host| {
            let rows = host.submenu().map_or_else(Vec::new, |menu| {
                menu.itemArray().iter().map(|item| bar_row(&item)).collect()
            });
            (host.title().to_string(), rows)
        })
        .collect()
}

fn bar_row(item: &NSMenuItem) -> BarRow {
    BarRow {
        title: item.title().to_string(),
        chord: chord(item),
        selector: item
            .action()
            .map_or_else(String::new, |sel| sel.name().to_string_lossy().into_owned()),
        separator: item.isSeparatorItem(),
        alternate: item.isAlternate(),
    }
}

fn chord(item: &NSMenuItem) -> String {
    let key = item.keyEquivalent().to_string();
    if key.is_empty() {
        return String::new();
    }
    let mask = item.keyEquivalentModifierMask();
    let mut out = String::new();
    if mask.contains(NSEventModifierFlags::Function) {
        out.push('🌐');
    }
    for (flag, glyph) in [
        (NSEventModifierFlags::Control, '⌃'),
        (NSEventModifierFlags::Option, '⌥'),
        (NSEventModifierFlags::Shift, '⇧'),
        (NSEventModifierFlags::Command, '⌘'),
    ] {
        if mask.contains(flag) {
            out.push(glyph);
        }
    }
    for ch in key.chars() {
        match ch {
            ' ' => out.push_str("Space"),
            '\r' => out.push('↩'),
            '\t' => out.push('⇥'),
            '\u{8}' | '\u{7f}' => out.push('⌫'),
            '\u{1b}' => out.push('⎋'),
            other => out.extend(other.to_uppercase()),
        }
    }
    out
}

fn menu_card(title: &str, rows: &[BarRow]) -> AnyElement {
    div()
        .w(px(268.))
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(system_color(SystemColor::Separator))
        .child(
            div()
                .text_sm()
                .text_color(system_color(SystemColor::Label))
                .child(SharedString::from(title.to_string())),
        )
        .children(rows.iter().map(bar_row_line))
        .into_any_element()
}

fn bar_row_line(row: &BarRow) -> AnyElement {
    if row.separator {
        return div()
            .my_1()
            .h(px(1.))
            .bg(system_color(SystemColor::Separator))
            .into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(system_color(if row.alternate {
                            SystemColor::TertiaryLabel
                        } else {
                            SystemColor::Label
                        }))
                        .child(SharedString::from(if row.alternate {
                            format!("{} · alt", row.title)
                        } else {
                            row.title.clone()
                        })),
                )
                .child(
                    div()
                        .font_family("SF Mono")
                        .text_xs()
                        .text_color(system_color(SystemColor::SecondaryLabel))
                        .child(SharedString::from(row.chord.clone())),
                ),
        )
        .child(
            div()
                .font_family("SF Mono")
                .text_xs()
                .text_color(system_color(SystemColor::TertiaryLabel))
                .child(SharedString::from(row.selector.clone())),
        )
        .into_any_element()
}
