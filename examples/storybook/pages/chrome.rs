//! Window chrome configures the window itself, so none of it can be shown inside the storybook's
//! content pane: the knobs here drive a second window opened through [`crate::popout`].
//!
//! Chrome is written from the knob handlers, never from [`Popout::render`]: installing a toolbar
//! reconfigures the window, which is not something to do to a window part-way through drawing it.

use crate::Demo;
use crate::ui::*;
use gpui::{
    AnyElement, App, Context, Entity, MouseMoveEvent, Pixels, Render, SharedString, WeakEntity,
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

/// Left of the storybook window, which opens at x=1180.
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

#[derive(Copy, Clone)]
struct Settings {
    radius: Option<Pixels>,
    toolbar: Option<ToolbarStyle>,
    level: WindowLevel,
    appearance: Option<Appearance>,
    shadow: bool,
    movable_by_background: bool,
    alpha: f32,
    paint_band: bool,
}

impl State {
    fn settings(&self) -> Settings {
        Settings {
            // Matches `set_corner_radius`'s own filter: a radius of 0 is "follow the system".
            radius: (self.radius > 0.).then(|| px(self.radius as f32)),
            toolbar: match self.toolbar {
                1 => Some(ToolbarStyle::Unified),
                2 => Some(ToolbarStyle::UnifiedCompact),
                _ => None,
            },
            level: LEVELS[self.level.min(LEVELS.len() - 1)].1,
            appearance: match self.appearance {
                1 => Some(Appearance::Light),
                2 => Some(Appearance::Dark),
                _ => None,
            },
            shadow: self.shadow,
            movable_by_background: self.movable_by_background,
            alpha: self.alpha as f32,
            paint_band: self.paint_band,
        }
    }

    fn push(&self, cx: &mut Context<Demo>) {
        let Some(handle) = self.window else { return };
        let demo = cx.weak_entity();
        let settings = self.settings();
        handle
            .update(cx, |popout: &mut Popout, window, cx| {
                popout.settings = settings;
                apply(&settings, &demo, window, cx);
                cx.notify();
            })
            .ok();
    }

    fn close(&mut self, cx: &mut App) {
        let Some(handle) = self.window.take() else {
            return;
        };
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }

    /// Puts the flags back to what the crate saw before it first touched the window, then reads
    /// them back into the knobs — otherwise the next [`State::push`] would undo the restore.
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

fn apply(settings: &Settings, demo: &WeakEntity<Demo>, window: &mut Window, cx: &mut App) {
    set_corner_radius(window, settings.radius);
    set_toolbar(window, cx, settings.toolbar, &toolbar_items(demo));

    let current = window_flags(window);
    let wanted = WindowFlags {
        has_shadow: settings.shadow,
        movable_by_background: settings.movable_by_background,
        alpha: settings.alpha,
        level: window_level(settings.level),
        appearance: settings.appearance,
        ..current
    };
    // `setAppearance:` with an equal-but-fresh NSAppearance would re-announce the change every
    // frame, so nothing is written unless something actually moved.
    if wanted != current {
        set_window_flags(window, wanted);
    }
}

/// The two wired items reach back into the page's own state, so the reader can see an AppKit-drawn
/// toolbar button move a GPUI control in the other window.
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

/// A toolbar action runs with its own window taken out of the app's window list, and
/// [`State::push`] has to update that window — so the write is deferred until it is back.
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

/// The popped-out window. It owns a copy of the knobs rather than reading the page's: `open_window`
/// draws synchronously, so the first draw lands while the caller still holds `Demo` leased, and
/// reading it from here would panic.
pub struct Popout {
    settings: Settings,
}

impl Popout {
    fn new(demo: &Entity<Demo>, settings: Settings, cx: &mut Context<Self>) -> Self {
        let demo = demo.downgrade();
        cx.on_release(move |_, cx| {
            demo.update(cx, |d: &mut Demo, cx| {
                d.chrome.window = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self { settings }
    }
}

impl Render for Popout {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.settings;

        let band = titlebar_band(window);
        let viewport = window.viewport_size();
        let flags = window_flags(window);
        let rows = vec![
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
                    .map(|inset| format!("{:.1}, {:.1}", f32::from(inset.x), f32::from(inset.y)))
                    .unwrap_or_else(|| "none".into()),
            ),
            line("level", flags.level.to_string()),
            line("has_shadow", flags.has_shadow.to_string()),
            line("is_opaque", flags.is_opaque.to_string()),
            line("alpha", format!("{:.2}", flags.alpha)),
            line(
                "titled / full_size_content",
                format!("{} / {}", flags.titled, flags.full_size_content),
            ),
            line("titlebar band painted", settings.paint_band.to_string()),
        ];

        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .top(if settings.paint_band { px(0.) } else { band })
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

fn fmt_radius(value: Option<Pixels>) -> String {
    value
        .map(fmt_pixels)
        .unwrap_or_else(|| "following the system".into())
}

pub fn render(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let open = d.chrome.window.is_some();
    let (radius, toolbar, level, appearance) = (
        d.chrome.radius,
        d.chrome.toolbar,
        d.chrome.level,
        d.chrome.appearance,
    );
    let (shadow, movable, paint_band, alpha) = (
        d.chrome.shadow,
        d.chrome.movable_by_background,
        d.chrome.paint_band,
        d.chrome.alpha,
    );

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
                        let demo = cx.entity();
                        let settings = d.chrome.settings();
                        d.chrome.window = crate::popout(cx, ORIGIN, EXTENT, {
                            let demo = demo.clone();
                            move |_, cx| Popout::new(&demo, settings, cx)
                        });
                        // Deferred so the first chrome write lands on a window that has been laid
                        // out: `titlebar_band` and the traffic-light inset are read from AppKit.
                        cx.defer(move |cx: &mut App| {
                            demo.update(cx, |d: &mut Demo, cx| {
                                d.chrome.push(cx);
                                cx.notify();
                            });
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
                })
                .into_any_element(),
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
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.chrome.radius = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "0 sends _setCornerRadius: 0, which AppKit reads as \"system default\" rather \
                     than square, and reads back as that default — so intent is tracked, never \
                     diffed against a readback. The second window prints both. Watch \
                     system_corner_radius while you switch the toolbar too: it is half the \
                     current band, so it follows the band's 32 / 40 / 52 to 16 / 20 / 26.",
                ),
                knob(
                    "toolbar",
                    TOOLBARS[toolbar.min(2)],
                    native_segmented("chrome.toolbar", toolbar)
                        .segments(TOOLBARS)
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.chrome.toolbar = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
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
                    LEVELS[level.min(LEVELS.len() - 1)].0,
                    native_popup("chrome.level", level)
                        .items(LEVELS.iter().map(|(name, _)| *name))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.chrome.level = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "window appearance",
                    APPEARANCES[appearance.min(2)],
                    native_segmented("chrome.appearance", appearance)
                        .segments(APPEARANCES)
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.chrome.appearance = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
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
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.chrome.shadow = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "movable by background",
                    if movable { "on" } else { "off" },
                    native_switch("chrome.movable", movable)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.chrome.movable_by_background = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "alpha",
                    format!("{alpha:.2}"),
                    native_slider("chrome.alpha", alpha)
                        .range(0.35, 1.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.chrome.alpha = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "alphaValue is the whole window, chrome included, and it composites against \
                     the desktop rather than against anything in the window. It stops at 0.35 \
                     here so the window stays findable — Restore puts it back with everything \
                     else.",
                ),
                caption(
                    "WindowFlags has two more this page deliberately leaves off a knob. Clearing \
                     titled removes the traffic lights, and this window has no other way to \
                     close; setting full_size_content is what window_options already does, and \
                     the crate's whole coordinate recipe assumes it. Both also shrink the frame \
                     on the way through, so they are install-time decisions rather than dials.",
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
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.chrome.paint_band = *value;
                            d.chrome.push(cx);
                            cx.notify();
                        }))
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
                        .map(|inset| {
                            format!("{:.1}, {:.1}", f32::from(inset.x), f32::from(inset.y))
                        })
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
                fmt_radius(d.chrome.settings().radius),
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
                window_level(LEVELS[level.min(LEVELS.len() - 1)].1).to_string(),
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
                "WindowFlags fields left off",
                "titled and full_size_content — both shrink the frame and move the traffic \
                 lights, so they are install-time, not dials"
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
    let swatches: Vec<AnyElement> = CURSORS
        .iter()
        .map(|(name, cursor)| {
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
            .into_any_element()
        })
        .collect();

    div()
        .flex()
        .flex_wrap()
        .gap_2()
        // So the readout below is the cursor as of the last pointer move, not as of the last
        // unrelated repaint.
        .on_mouse_move(cx.listener(|_: &mut Demo, _: &MouseMoveEvent, _, cx| cx.notify()))
        .children(swatches)
        .into_any_element()
}
