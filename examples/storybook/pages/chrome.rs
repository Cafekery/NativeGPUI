use crate::Demo;
use crate::ui::*;
use gpui::{
    AnyElement, App, Context, MouseMoveEvent, Pixels, Point, Render, SharedString, WeakEntity,
    Window, WindowHandle, div, prelude::*, px,
};
use native_gpui::tokens::{Appearance, SystemColor, system_color};
use native_gpui::{
    Cursor, ToolbarItem, ToolbarStyle, WindowFlags, WindowLevel, corner_radius,
    corner_radius_readback, current_cursor_description, cursor_region, native_button, native_popup,
    native_segmented, native_slider, native_switch, restore_window_flags, set_corner_radius,
    set_toolbar, set_window_flags, supports_corner_radius, system_corner_radius, titlebar_band,
    traffic_light_inset, window_flags, window_level,
};

const ORIGIN: (f32, f32) = (300., 240.);
const EXTENT: (f32, f32) = (640., 470.);

const TOOLBARS: [&str; 3] = ["none", "unified", "compact"];
const APPEARANCES: [&str; 3] = ["system", "light", "dark"];

const LEVELS: [(&str, WindowLevel); 7] = [
    ("Normal", WindowLevel::Normal),
    ("Floating", WindowLevel::Floating),
    ("TornOffMenu", WindowLevel::TornOffMenu),
    ("ModalPanel", WindowLevel::ModalPanel),
    ("PopUpMenu", WindowLevel::PopUpMenu),
    ("Status", WindowLevel::Status),
    ("ScreenSaver", WindowLevel::ScreenSaver),
];

const CURSORS: [(&str, Cursor); 6] = [
    ("PointingHand", Cursor::PointingHand),
    ("IBeam", Cursor::IBeam),
    ("Crosshair", Cursor::Crosshair),
    ("OpenHand", Cursor::OpenHand),
    ("ColumnResize", Cursor::ColumnResize),
    ("OperationNotAllowed", Cursor::OperationNotAllowed),
];

const RADIUS_CYCLE: [f64; 4] = [0., 12., 24., 40.];

pub struct State {
    pub window: Option<WindowHandle<Popout>>,
    pub radius: f64,
    pub toolbar: usize,
    pub level: usize,
    pub appearance: usize,
    pub shadow: bool,
    pub movable_by_background: bool,
    pub alpha: f64,
    pub paint_band: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            window: None,
            radius: 24.,
            toolbar: 1,
            level: 0,
            appearance: 0,
            shadow: true,
            movable_by_background: true,
            alpha: 1.,
            paint_band: true,
        }
    }
}

impl State {
    // A radius of 0 means "follow the system", not "square".
    fn corner_radius(&self) -> Option<Pixels> {
        (self.radius > 0.).then(|| px(self.radius as f32))
    }

    fn level(&self) -> (&'static str, WindowLevel) {
        LEVELS[self.level.min(LEVELS.len() - 1)]
    }

    fn push(&self, cx: &mut Context<Demo>) {
        let Some(handle) = self.window else { return };
        let demo = cx.weak_entity();
        handle
            .update(cx, |popout: &mut Popout, window, cx| {
                popout.paint_band = self.paint_band;
                self.apply(&demo, window, cx);
                cx.notify();
            })
            .ok();
    }

    fn apply(&self, demo: &WeakEntity<Demo>, window: &mut Window, cx: &mut App) {
        set_corner_radius(window, self.corner_radius());
        let toolbar = match self.toolbar {
            1 => Some(ToolbarStyle::Unified),
            2 => Some(ToolbarStyle::UnifiedCompact),
            _ => None,
        };
        set_toolbar(window, cx, toolbar, &toolbar_items(demo));

        let current = window_flags(window);
        let wanted = WindowFlags {
            has_shadow: self.shadow,
            movable_by_background: self.movable_by_background,
            alpha: self.alpha as f32,
            level: window_level(self.level().1),
            appearance: match self.appearance {
                1 => Some(Appearance::Light),
                2 => Some(Appearance::Dark),
                _ => None,
            },
            ..current
        };
        // `setAppearance:` with an equal-but-fresh NSAppearance re-announces the change every frame.
        if wanted != current {
            set_window_flags(window, wanted);
        }
    }

    fn close(&mut self, cx: &mut App) {
        if let Some(handle) = self.window.take() {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    }

    // The knobs are re-read from the window, or the next `push` would undo the restore.
    fn restore(&mut self, cx: &mut App) {
        let Some(handle) = self.window else { return };
        let Ok(flags) = handle.update(cx, |_, window, cx| {
            restore_window_flags(window);
            cx.notify();
            window_flags(window)
        }) else {
            return;
        };
        self.shadow = flags.has_shadow;
        self.movable_by_background = flags.movable_by_background;
        self.alpha = flags.alpha as f64;
        self.level = LEVELS
            .iter()
            .position(|(_, level)| window_level(*level) == flags.level)
            .unwrap_or(0);
        self.appearance = match flags.appearance {
            None => 0,
            Some(Appearance::Light) => 1,
            Some(Appearance::Dark) => 2,
        };
    }
}

fn change<'a, T: Copy + 'static>(
    cx: &'a mut Context<Demo>,
    apply: fn(&mut State, T),
) -> impl Fn(&T, &mut Window, &mut App) + 'static + use<'a, T> {
    cx.listener(move |d: &mut Demo, value: &T, _, cx| {
        apply(&mut d.chrome, *value);
        d.chrome.push(cx);
        cx.notify();
    })
}

fn toolbar_items(demo: &WeakEntity<Demo>) -> Vec<ToolbarItem> {
    vec![
        ToolbarItem::symbol("chrome.sidebar", "sidebar.left", "Sidebar"),
        ToolbarItem::flexible_space(),
        ToolbarItem::symbol(
            "chrome.band",
            "rectangle.tophalf.inset.filled",
            "Titlebar band",
        )
        .on_click(edit(demo.clone(), |state| {
            state.paint_band = !state.paint_band
        })),
        ToolbarItem::symbol("chrome.radius", "circle.dashed", "Corner radius").on_click(edit(
            demo.clone(),
            |state| {
                let next = RADIUS_CYCLE
                    .iter()
                    .position(|step| *step > state.radius)
                    .unwrap_or(0);
                state.radius = RADIUS_CYCLE[next];
            },
        )),
    ]
}

// A toolbar action runs with its own window off the app's window list; defer until it is back.
fn edit(demo: WeakEntity<Demo>, change: fn(&mut State)) -> impl Fn(&mut Window, &mut App) {
    move |_, cx| {
        let demo = demo.clone();
        cx.defer(move |cx| {
            demo.update(cx, |d: &mut Demo, cx| {
                change(&mut d.chrome);
                d.chrome.push(cx);
                cx.notify();
            })
            .ok();
        });
    }
}

// Copies the knob: `open_window` draws while `Demo` is still leased.
pub struct Popout {
    paint_band: bool,
}

impl Popout {
    fn new(demo: WeakEntity<Demo>, paint_band: bool, cx: &mut Context<Self>) -> Self {
        cx.on_release(move |_, cx| {
            demo.update(cx, |d: &mut Demo, cx| {
                d.chrome.window = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self { paint_band }
    }
}

impl Render for Popout {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let band = titlebar_band(window);
        let viewport = window.viewport_size();
        let flags = window_flags(window);
        let rows = [
            line("corner_radius (intent)", fmt_radius(corner_radius(window))),
            line(
                "_cornerRadius (readback)",
                fmt_radius(corner_radius_readback(window)),
            ),
            line(
                "system_corner_radius",
                fmt_pixels(system_corner_radius(window)),
            ),
            line(
                "supports_corner_radius",
                supports_corner_radius(window).to_string(),
            ),
            line("titlebar_band", fmt_pixels(band)),
            line(
                "viewport",
                format!(
                    "{:.0} x {:.0}",
                    f32::from(viewport.width),
                    f32::from(viewport.height)
                ),
            ),
            line(
                "viewport below the band",
                fmt_pixels(viewport.height - band),
            ),
            line(
                "traffic_light_inset",
                traffic_light_inset(window)
                    .map(fmt_inset)
                    .unwrap_or_else(|| "none".into()),
            ),
            line("level", flags.level.to_string()),
            line("has_shadow", flags.has_shadow.to_string()),
            line("is_opaque", flags.is_opaque.to_string()),
            line("alpha", format!("{:.2}", flags.alpha)),
            line("titled", flags.titled.to_string()),
            line("full_size_content", flags.full_size_content.to_string()),
            line(
                "movable_by_background",
                flags.movable_by_background.to_string(),
            ),
            line(
                "titlebar_appears_transparent",
                flags.titlebar_appears_transparent.to_string(),
            ),
            line("collection", format!("{:#x}", flags.collection.0)),
            line(
                "appearance",
                match flags.appearance {
                    Some(a) => format!("{a:?}"),
                    None => "follows the system".into(),
                },
            ),
            line("titlebar band painted", self.paint_band.to_string()),
        ];

        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .top(if self.paint_band { px(0.) } else { band })
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(system_color(SystemColor::WindowBackground)),
            )
            .child(
                div()
                    .absolute()
                    .top(band + px(20.))
                    .left(px(26.))
                    .right(px(26.))
                    .bottom(px(20.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .text_color(system_color(SystemColor::Label))
                            .child("Second window"),
                    )
                    .child(div().pb_2().child(caption(
                        "read from this window every frame — the storybook's knobs write it",
                    )))
                    .children(rows),
            )
            .into_any_element()
    }
}

fn line(name: &str, value: String) -> AnyElement {
    mono(format!("{name:<26}{value}"))
}

fn fmt_pixels(value: Pixels) -> String {
    format!("{:.1}", f32::from(value))
}

fn fmt_inset(inset: Point<Pixels>) -> String {
    format!("{:.1}, {:.1}", f32::from(inset.x), f32::from(inset.y))
}

fn fmt_radius(value: Option<Pixels>) -> String {
    value
        .map(fmt_pixels)
        .unwrap_or_else(|| "following the system".into())
}

pub fn render(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let State {
        radius,
        toolbar,
        appearance,
        shadow,
        movable_by_background: movable,
        alpha,
        paint_band,
        ..
    } = d.chrome;
    let open = d.chrome.window.is_some();
    let level = d.chrome.level();

    stack(vec![
        note(
            "Every knob on this page writes the window itself, so it needs a window of its own. \
             Open the second window, then leave it visible while you drive it from here.",
        ),
        row(
            "second window",
            vec![
                native_button("chrome.open", "Open")
                    .enabled(!open)
                    .on_click(cx.listener(|d: &mut Demo, _: &(), _, cx| {
                        let demo = cx.weak_entity();
                        let paint_band = d.chrome.paint_band;
                        d.chrome.window = crate::popout(cx, ORIGIN, EXTENT, {
                            let demo = demo.clone();
                            move |_, cx| Popout::new(demo, paint_band, cx)
                        });
                        // AppKit reports the band and inset only once the window is laid out.
                        cx.defer(move |cx| {
                            demo.update(cx, |d: &mut Demo, cx| {
                                d.chrome.push(cx);
                                cx.notify();
                            })
                            .ok();
                        });
                        cx.notify();
                    }))
                    .into_any_element(),
                native_button("chrome.close", "Close")
                    .enabled(open)
                    .on_click(cx.listener(|d: &mut Demo, _: &(), _, cx| {
                        d.chrome.close(cx);
                        cx.notify();
                    }))
                    .into_any_element(),
                native_button("chrome.restore", "Restore flags")
                    .enabled(open)
                    .on_click(cx.listener(|d: &mut Demo, _: &(), _, cx| {
                        d.chrome.restore(cx);
                        cx.notify();
                    }))
                    .into_any_element(),
                mono(if open {
                    "open"
                } else {
                    "closed — the knobs below have nothing to write"
                }),
            ],
        ),
        section(
            "the window's own chrome",
            vec![
                knob(
                    "corner radius",
                    if radius > 0. {
                        format!("{radius:.0}")
                    } else {
                        "system".into()
                    },
                    native_slider("chrome.radius", radius)
                        .range(0., 40.)
                        .ticks(21)
                        .snap_to_ticks(true)
                        .on_change(change(cx, |state, value| state.radius = value))
                        .into_any_element(),
                ),
                caption(
                    "0 sends _setCornerRadius: 0, which AppKit reads as \"system default\" rather \
                     than square, and reads back as that default — so intent is tracked, never \
                     diffed against a readback. The second window prints both.",
                ),
                caption(
                    "system_corner_radius is half the current band, so it would follow the \
                     band's 32 / 40 / 52 to 16 / 20 / 26 — but only while nothing has been \
                     written. Once set_corner_radius has run it reports the last value it saw \
                     and stops tracking, which is why the second window reads 16 under a 52pt \
                     band while the properties table below, on a window nothing has written, \
                     reads 26. Do not read it as a live measure of the band; read titlebar_band.",
                ),
                knob(
                    "toolbar",
                    TOOLBARS[toolbar.min(2)],
                    native_segmented("chrome.toolbar", toolbar)
                        .segments(TOOLBARS)
                        .on_change(change(cx, |state, value| state.toolbar = value))
                        .into_any_element(),
                ),
                caption(format!(
                    "A toolbar takes GPUI content rather than giving any: the frame does not \
                     move, so the band grows 32 → 40 (compact) → 52 (unified) while the viewport \
                     stays put, and the space below it drops {:.0} → {:.0} → {:.0} at this \
                     window's {:.0}pt height. The band figures are contentLayoutRect losing 8pt \
                     to compact and 20pt to unified, measured on a bare 32pt band. Under \
                     FullSizeContentView the content rect is the frame, so AppKit has no content \
                     height to preserve by growing it. Inset your own content by titlebar_band, \
                     which the second window prints live.",
                    EXTENT.1 - 32.,
                    EXTENT.1 - 40.,
                    EXTENT.1 - 52.,
                    EXTENT.1
                )),
                knob(
                    "window level",
                    level.0,
                    native_popup("chrome.level", d.chrome.level)
                        .items(LEVELS.iter().map(|(name, _)| *name))
                        .on_change(change(cx, |state, value| state.level = value))
                        .into_any_element(),
                ),
                knob(
                    "window appearance",
                    APPEARANCES[appearance.min(2)],
                    native_segmented("chrome.appearance", appearance)
                        .segments(APPEARANCES)
                        .on_change(change(cx, |state, value| state.appearance = value))
                        .into_any_element(),
                ),
                caption(
                    "Window-scoped, and it only moves what AppKit draws — the titlebar, the \
                     toolbar and the traffic lights. GPUI's own colours come from \
                     tokens::set_app_appearance instead.",
                ),
                knob(
                    "has shadow",
                    if shadow { "on" } else { "off" },
                    native_switch("chrome.shadow", shadow)
                        .on_change(change(cx, |state, value| state.shadow = value))
                        .into_any_element(),
                ),
                knob(
                    "movable by background",
                    if movable { "on" } else { "off" },
                    native_switch("chrome.movable", movable)
                        .on_change(change(cx, |state, value| {
                            state.movable_by_background = value
                        }))
                        .into_any_element(),
                ),
                knob(
                    "alpha",
                    format!("{alpha:.2}"),
                    native_slider("chrome.alpha", alpha)
                        .range(0.35, 1.)
                        .on_change(change(cx, |state, value| state.alpha = value))
                        .into_any_element(),
                ),
                caption(
                    "alphaValue is the whole window, chrome included, and it composites against \
                     the desktop rather than against anything in the window. It stops at 0.35 \
                     here so the window stays findable — Restore puts it back with everything \
                     else.",
                ),
                caption(
                    "Five of WindowFlags' ten fields are left off a knob here, for two different \
                     reasons. titled and full_size_content are install-time: clearing titled \
                     removes the traffic lights and this window has no other way to close, \
                     setting full_size_content is what window_options already does and the \
                     crate's whole coordinate recipe assumes it, and both shrink the frame on the \
                     way through. is_opaque, titlebar_appears_transparent and collection are left \
                     off because window_options and the sidebar own them — a dial that fought \
                     those would be demonstrating this page rather than the crate. All ten are \
                     read back in the second window.",
                ),
            ],
        ),
        section(
            "an unpainted titlebar band",
            vec![
                knob(
                    "paint the band",
                    if paint_band { "painted" } else { "unpainted" },
                    native_switch("chrome.band", paint_band)
                        .on_change(change(cx, |state, value| state.paint_band = value))
                        .into_any_element(),
                ),
                note(
                    "Turn this off to inset content by titlebar_band and paint nothing behind it — \
                     the usual mistake. The window is transparent, so with no toolbar the band \
                     really does go empty: the whole top 32pt reads alpha 0 across the full width, \
                     traffic lights aside. Install a toolbar first and it does not, because the \
                     toolbar's own backing fills all but the top 8pt — which makes a toolbar the \
                     wrong arm to test this in. The drop shadow does not care either way. It stays \
                     byte-identical, corner arc included: the silhouette follows the frame and its \
                     radius, not the painted content, whether _setCornerRadius: was given a value, \
                     given 0, or never called at all. Turning has shadow off is what moves it, and \
                     so does changing the radius.",
                ),
            ],
        ),
        section(
            "traffic lights",
            vec![
                caption(
                    "set_toolbar is mutually exclusive with TitlebarOptions::traffic_light_position. \
                     With a position set, GPUI's move_traffic_light() hard-sets all three button \
                     frames on open, activation and fullscreen, overwriting the insets AppKit \
                     applied for the toolbar; nothing warns, the buttons just sit wrong. The \
                     storybook's popout leaves the position unset, so switching the toolbar on \
                     moves traffic_light_inset — the second window prints it.",
                ),
                line(
                    "this window's inset",
                    traffic_light_inset(window)
                        .map(fmt_inset)
                        .unwrap_or_else(|| "none — no close button".into()),
                ),
                line("this window's band", fmt_pixels(titlebar_band(window))),
            ],
        ),
        section(
            "window levels, asked of CGWindowLevelForKey",
            LEVELS
                .iter()
                .map(|(name, level)| line(name, window_level(*level).to_string()))
                .collect(),
        ),
        caption("Several names share a number, so a level is not a discriminator for a window."),
        section(
            "cursor regions",
            vec![
                caption(
                    "A GPUI element wearing a real system cursor. Both mechanisms are driven: an \
                     AppKit cursor rect alone loses to GPUI's own reset_cursor_style at the end of \
                     every frame.",
                ),
                cursor_strip(cx),
                line("NSCursor now", current_cursor_description()),
            ],
        ),
        props(&[
            (
                "set_corner_radius(&Window, Option<Pixels>)",
                fmt_radius(d.chrome.corner_radius()),
            ),
            (
                "corner_radius / corner_radius_readback",
                "intent vs live".into(),
            ),
            (
                "supports_corner_radius / system_corner_radius",
                format!(
                    "{} · {}",
                    supports_corner_radius(window),
                    fmt_pixels(system_corner_radius(window))
                ),
            ),
            (
                "set_toolbar(.., Option<ToolbarStyle>, &[ToolbarItem])",
                TOOLBARS[toolbar.min(2)].into(),
            ),
            (
                "ToolbarItem::symbol / flexible_space / space",
                "two wired; the third has no on_click, so AppKit greys it out".into(),
            ),
            ("titlebar_band(&Window)", fmt_pixels(titlebar_band(window))),
            (
                "traffic_light_inset(&Window)",
                "printed in the second window".into(),
            ),
            (
                "window_level(WindowLevel)",
                window_level(level.1).to_string(),
            ),
            (
                "window_flags / set_window_flags / restore_window_flags",
                "read, change one field, write back".into(),
            ),
            (
                "WindowFlags fields on a knob",
                format!(
                    "has_shadow · movable_by_background · alpha {alpha:.2} · level · appearance"
                ),
            ),
            (
                "WindowFlags fields left off a knob",
                "titled · full_size_content — install-time, they shrink the frame and move the \
                 traffic lights; is_opaque · titlebar_appears_transparent · collection — owned by \
                 window_options and the sidebar. All ten are read back in the second window"
                    .into(),
            ),
            (
                "cursor_region(id, Cursor, child)",
                format!("{} regions", CURSORS.len()),
            ),
            ("current_cursor_description()", current_cursor_description()),
        ]),
    ])
}

fn cursor_strip(cx: &mut Context<Demo>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .on_mouse_move(cx.listener(|_: &mut Demo, _: &MouseMoveEvent, _, cx| cx.notify()))
        .children(CURSORS.iter().map(|(name, cursor)| {
            cursor_region(
                *name,
                *cursor,
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(system_color(SystemColor::ControlBackground))
                    .border_1()
                    .border_color(system_color(SystemColor::Separator))
                    .text_xs()
                    .text_color(system_color(SystemColor::SecondaryLabel))
                    .child(SharedString::from(*name)),
            )
        }))
        .into_any_element()
}
