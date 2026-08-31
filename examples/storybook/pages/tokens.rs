//! The system's own colours, fonts and symbols — the parts GPUI draws itself.
//!
//! Every swatch is split Light | Dark, resolved through [`SystemColor::try_resolve`] rather than the
//! live appearance, because most of these colours differ between the two worlds: a single-column
//! grid would document one appearance and quietly contradict the other.
//!
//! The appearance switch is a real `NSSwitch` driving [`set_app_appearance`], which flips the whole
//! process through the same KVO a system change does. That is a side effect outside this page, so
//! leaving the page drops the override — see [`watch_for_leave`].

use crate::Demo;
use crate::ui::*;
use gpui::{
    AnyElement, App, Context, FontWeight, Hsla, Pixels, Rgba, Subscription, Window, div,
    prelude::*, px,
};
use native_gpui::tokens::{
    Appearance, SymbolColor, SystemColor, SystemTheme, accent, advance_delta, appkit_advance,
    appkit_string_width, cache_stats, current_appearance, generation, label_font_size, refused,
    set_app_appearance, sf_symbol, sf_symbol_colored, small_system_font_size, srgb, system_color,
    system_font, system_font_family, system_font_metrics, system_font_size, system_font_size_for,
    text_font_family,
};
use native_gpui::{ControlSize, native_button, native_switch};
use objc2_app_kit::NSColor;

/// Ascenders, descenders, digits and a repeated pair, so cumulative advance drift is visible.
const SAMPLE: &str = "Handgloves 0123 xnxn";

const SIZES: &[f32] = &[9., 11., 13., 16., 17., 24.];

const SYMBOLS: &[(&str, fn() -> SymbolColor)] = &[
    ("gearshape", || SymbolColor::Label),
    ("magnifyingglass", || SymbolColor::Label),
    ("trash", || SymbolColor::Label),
    ("wifi", || SymbolColor::Label),
    ("folder.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemTeal)
    }),
    ("bolt.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemOrange)
    }),
    ("checkmark.seal.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemGreen)
    }),
    ("star.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemYellow)
    }),
    ("bell.badge", || {
        SymbolColor::Palette(vec![SystemColor::SystemRed, SystemColor::SystemGray])
    }),
    ("externaldrive.fill.badge.wifi", || {
        SymbolColor::Palette(vec![SystemColor::SystemBlue, SystemColor::SystemGray])
    }),
    ("paintpalette.fill", || SymbolColor::Multicolor),
    ("cloud.sun.rain.fill", || SymbolColor::Multicolor),
    ("battery.75percent", || SymbolColor::Raw),
    ("lock.fill", || SymbolColor::Raw),
];

/// A name no release ships, to show [`sf_symbol`] answers `None` rather than a placeholder glyph.
const ABSENT_SYMBOL: &str = "definitely.not.a.symbol";

#[derive(Default)]
pub struct State {
    /// Whether *this page* installed the process-wide appearance override.
    pinned: bool,
    /// Whether a leave-watching frame callback is already armed.
    watching: bool,
    theme: Option<Subscription>,
    /// Light-vs-dark difference count, keyed by the token generation it was measured in.
    differing: Option<(u64, usize)>,
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    if d.tokens.theme.is_none() {
        d.tokens.theme = Some(cx.observe_global::<SystemTheme>(|_, cx| cx.notify()));
    }

    let token_generation = generation();
    if d.tokens.differing.map(|(g, _)| g) != Some(token_generation) {
        let differing = SystemColor::ALL
            .iter()
            .filter(|c| c.try_resolve(Appearance::Light) != c.try_resolve(Appearance::Dark))
            .count();
        d.tokens.differing = Some((token_generation, differing));
    }
    let differing = d.tokens.differing.map_or(0, |(_, n)| n);
    let pinned = d.tokens.pinned;

    let fonts = font_section(cx);
    let symbols = symbol_section();
    let colors = color_section(differing);
    let appearance = appearance_section(pinned, cx);

    stack(vec![
        appearance,
        colors,
        fonts,
        symbols,
        cache_section(),
        props(&[
            (
                "system_color(SystemColor)",
                format!("{} semantic colours", SystemColor::ALL.len()),
            ),
            (
                "SystemColor::try_resolve(Appearance)",
                format!("{differing} of {} differ", SystemColor::ALL.len()),
            ),
            ("accent()", hex8(accent())),
            (
                "current_appearance()",
                format!("{:?}", current_appearance()),
            ),
            (
                "set_app_appearance(Option<_>)",
                if pinned {
                    "overridden by this page".into()
                } else {
                    "None — following the system".into()
                },
            ),
            ("system_font(FontWeight)", system_font_family().into()),
            (
                "system_font_metrics(size, weight)",
                system_font_metrics(system_font_size(), FontWeight::NORMAL).resolved_name,
            ),
            (
                "advance_delta(cx, family, ..)",
                "GPUI minus AppKit, in points".into(),
            ),
            (
                "sf_symbol(name, size, weight)",
                format!("{} rasterised, 1 absent", SYMBOLS.len()),
            ),
            ("generation() / cache_stats()", token_generation.to_string()),
        ]),
    ])
}

/// The appearance override is process-wide, and `render` runs only while this page is showing —
/// so a frame callback is the only hook that fires on the frame that draws a *different* page.
/// Without it the user is left pinned to an appearance they never asked for.
fn watch_for_leave(window: &mut Window, cx: &mut Context<Demo>) {
    cx.on_next_frame(window, |d: &mut Demo, window, cx| {
        if !d.tokens.pinned {
            d.tokens.watching = false;
        } else if d.selected.as_ref() == "tokens" {
            watch_for_leave(window, cx);
        } else {
            d.tokens.pinned = false;
            d.tokens.watching = false;
            set_app_appearance(None);
            cx.notify();
        }
    });
}

fn appearance_section(pinned: bool, cx: &mut Context<Demo>) -> AnyElement {
    let live = current_appearance();
    let theme = cx.try_global::<SystemTheme>().copied();

    let switch = native_switch("appearance", live == Appearance::Dark)
        .on_change(cx.listener(|d: &mut Demo, dark: &bool, window, cx| {
            d.tokens.pinned = true;
            set_app_appearance(Some(if *dark {
                Appearance::Dark
            } else {
                Appearance::Light
            }));
            if !d.tokens.watching {
                d.tokens.watching = true;
                watch_for_leave(window, cx);
            }
            cx.notify();
        }))
        .into_any_element();

    let follow = native_button("follow-system", "Follow system")
        .enabled(pinned)
        .on_click(cx.listener(|d: &mut Demo, _: &(), _, cx| {
            d.tokens.pinned = false;
            set_app_appearance(None);
            cx.notify();
        }))
        .into_any_element();

    section(
        "appearance",
        vec![
            knob("set_app_appearance(Dark)", format!("{live:?}"), switch),
            knob(
                "set_app_appearance(None)",
                if pinned { "overridden" } else { "system" },
                follow,
            ),
            mono(format!(
                "current_appearance() = {live:?}    SystemTheme = {}",
                theme.map_or_else(
                    || "not published".to_string(),
                    |t| format!(
                        "{:?}, accent {}, generation {}",
                        t.appearance,
                        hex8(t.accent),
                        t.generation
                    )
                )
            )),
            note(
                "Flipping this switch calls set_app_appearance, which repaints the whole process — \
                 sidebar and window chrome included. Leaving this page restores None, so the app \
                 goes back to following the system rather than staying pinned.",
            ),
        ],
    )
}

fn color_section(differing: usize) -> AnyElement {
    let total = SystemColor::ALL.len();
    let tiles: Vec<AnyElement> = SystemColor::ALL.iter().copied().map(color_tile).collect();

    let (light_refused, dark_refused) = (refused(Appearance::Light), refused(Appearance::Dark));
    let pattern = srgb(&NSColor::scrubberTexturedBackgroundColor());

    section(
        "semantic colours",
        vec![
            caption(format!(
                "{total} colours, each swatch Light | Dark over a checkerboard. \
                 {differing} of {total} differ between the two, so a swatch is only meaningful \
                 with its appearance stated."
            )),
            grid(tiles),
            demo(
                "colours that refuse sRGB conversion",
                stack(vec![
                    mono(format!(
                        "refused(Light) {} of {total}    refused(Dark) {} of {total}{}",
                        light_refused.len(),
                        dark_refused.len(),
                        if light_refused.is_empty() && dark_refused.is_empty() {
                            String::new()
                        } else {
                            format!("    {light_refused:?}    {dark_refused:?}")
                        }
                    )),
                    mono(format!(
                        "srgb(NSColor.scrubberTexturedBackgroundColor) = {}",
                        match pattern {
                            Some(c) => hex8(c),
                            None => "None".into(),
                        }
                    )),
                    note(
                        "The pattern colour on the second line is the control: an empty refusal \
                         list is only evidence if something that can refuse is measured beside \
                         it. scrubberTexturedBackground is an NSPatternColor and is absent from \
                         SystemColor, so it never reaches the grid above. sRGB conversion is not \
                         defined for a pattern colour, which is why try_resolve has to keep \
                         returning an Option — the value printed beside it is whatever this OS \
                         actually answered, not one this page assumed. Reading redComponent off \
                         one instead of going through colorUsingColorSpace either throws or \
                         yields a plausible-looking black, and a black swatch up there would have \
                         looked entirely correct.",
                    ),
                ]),
            ),
        ],
    )
}

fn color_tile(c: SystemColor) -> AnyElement {
    let (light, dark) = (
        c.try_resolve(Appearance::Light),
        c.try_resolve(Appearance::Dark),
    );
    let half = |value: Option<Hsla>| {
        let cell = div().flex_1().h_full();
        match value {
            Some(value) => cell.bg(value),
            None => cell,
        }
    };

    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .w(px(116.))
        .child(checker(
            116.,
            div()
                .flex()
                .size_full()
                .child(half(light))
                .child(half(dark))
                .into_any_element(),
        ))
        .child(mono(format!(
            "{} {c:?}",
            if light == dark { "=" } else { "≠" }
        )))
        .child(caption(format!("{} {}", opt_hex(light), opt_hex(dark))))
        .into_any_element()
}

fn font_section(cx: &App) -> AnyElement {
    let base = system_font_size();
    let weight = FontWeight::NORMAL;

    let control_sizes = [
        ("Mini", ControlSize::Mini),
        ("Small", ControlSize::Small),
        ("Regular", ControlSize::Regular),
        ("Large", ControlSize::Large),
        ("ExtraLarge", ControlSize::ExtraLarge),
    ]
    .map(|(name, size)| format!("{name}={:.0}", f32::from(system_font_size_for(size))))
    .join("  ");

    let ramp: Vec<AnyElement> = SIZES
        .iter()
        .map(|&size| {
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .child(sized(40., caption(format!("{size:.0}pt"))))
                .child(
                    div()
                        .font(system_font(weight))
                        .text_size(px(size))
                        .text_color(system_color(SystemColor::Label))
                        .child(SAMPLE),
                )
                .into_any_element()
        })
        .collect();

    let weights: Vec<AnyElement> = FontWeight::ALL
        .iter()
        .map(|&w| {
            let metrics = system_font_metrics(base, w);
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .child(sized(52., caption(format!("{:.0}", w.0))))
                .child(
                    div()
                        .w(px(200.))
                        .font(system_font(w))
                        .text_size(base)
                        .text_color(system_color(SystemColor::Label))
                        .child(SAMPLE),
                )
                .child(caption(metrics.resolved_name))
                .into_any_element()
        })
        .collect();

    let advances: Vec<AnyElement> = SIZES
        .iter()
        .map(|&size| {
            let at = px(size);
            let appkit = appkit_advance(system_font_family(), at, weight, 'x');
            let delta = |family| {
                advance_delta(cx, family, at, weight, 'x')
                    .map_or_else(|| "n/a".to_string(), |d| format!("{d:+.4}"))
            };
            mono(format!(
                "{size:>4.0}pt  AppKit {:>8.4}   system {:>9}   Menlo {:>9}",
                appkit.unwrap_or(f32::NAN),
                delta(system_font_family()),
                delta("Menlo"),
            ))
        })
        .collect();

    let text_system = cx.text_system();
    let font_id = text_system.resolve_font(&system_font(weight));
    let drift: Vec<AnyElement> = SIZES
        .iter()
        .map(|&size| {
            let appkit = appkit_string_width(system_font_family(), px(size), weight, SAMPLE);
            let gpui: f32 = SAMPLE
                .chars()
                .filter_map(|ch| text_system.advance(font_id, px(size), ch).ok())
                .map(|a| f32::from(a.width))
                .sum();
            mono(match appkit {
                Some(appkit) => format!(
                    "{size:>4.0}pt  AppKit {appkit:>8.3}pt  GPUI {gpui:>8.3}px  {:+.3}pt ({:+.2}%)",
                    gpui - appkit,
                    100. * (gpui - appkit) / appkit
                ),
                None => format!("{size:>4.0}pt  AppKit n/a"),
            })
        })
        .collect();

    let metrics = system_font_metrics(base, weight);

    section(
        "system font",
        vec![
            mono(format!(
                "system_font_family() = {}    text_font_family() = {}",
                system_font_family(),
                text_font_family(cx).unwrap_or("None — SF Pro Text is not installed")
            )),
            mono(format!(
                "systemFontSize {:.0}    smallSystemFontSize {:.0}    labelFontSize {:.0}",
                f32::from(base),
                f32::from(small_system_font_size()),
                f32::from(label_font_size())
            )),
            demo(
                "system_font_size_for(ControlSize)",
                stack(vec![
                    mono(control_sizes),
                    caption(
                        "Five sizes, but AppKit does not answer with five distinct values — the \
                         crate documents 13pt for Regular, Large and ExtraLarge alike, and the \
                         row above is what this machine answered. Hardcoding 15 or 17 puts a GPUI \
                         label out of step with the control beside it.",
                    ),
                ]),
            ),
            demo("sizes", stack(ramp)),
            demo(
                "FontWeight::ALL, with the face AppKit resolved",
                stack(weights),
            ),
            demo(
                "advance of 'x', GPUI minus AppKit, in points",
                stack(vec![
                    stack(advances),
                    caption(
                        "GPUI reaches the right face but does not apply SF's optical-size (opsz) \
                         axis, which CoreText applies whenever you ask NSFont for a size. The \
                         crate documents GPUI about 2.6% narrow at 13pt, exact around 16pt and \
                         wider above — the row above is what this machine answered, so compare \
                         the two rather than trusting either alone. Menlo is the control, not \
                         decoration: if it disagrees too then the two instruments are \
                         incomparable and the system column is noise rather than a gap.",
                    ),
                ]),
            ),
            demo("cumulative drift over the sample string", stack(drift)),
            mono(format!(
                "metrics at {:.0}pt: ascent {:+.3} descent {:+.3} leading {:.3} cap {:.3} \
                 x-height {:.3}  face {}",
                f32::from(base),
                metrics.ascent,
                metrics.descent,
                metrics.leading,
                metrics.cap_height,
                metrics.x_height,
                metrics.resolved_name
            )),
        ],
    )
}

/// Symbols are asked for fresh on every render rather than held in `State`: a `Symbol` kept across
/// an appearance change is a stale raster, and a `labelColor` one goes invisible.
fn symbol_section() -> AnyElement {
    let base = system_font_size();
    let weight = FontWeight::NORMAL;

    let catalogue: Vec<AnyElement> = SYMBOLS
        .iter()
        .map(|(name, color)| {
            let color = color();
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_1p5()
                .w(px(116.))
                .child(symbol_cell(name, px(22.), weight, &color))
                .child(mono(*name))
                .child(caption(symbol_color_name(&color)))
                .into_any_element()
        })
        .collect();

    let sizes: Vec<AnyElement> = SIZES
        .iter()
        .map(|&size| symbol_cell("slider.horizontal.3", px(size), weight, &SymbolColor::Label))
        .collect();

    let weights: Vec<AnyElement> = FontWeight::ALL
        .iter()
        .map(|&w| symbol_cell("cloud.fill", px(22.), w, &SymbolColor::Label))
        .collect();

    section(
        "SF Symbols",
        vec![
            caption(
                "Rasterised by AppKit and handed to GPUI as a RenderImage. Each sits on a \
                 checkerboard because a SymbolColor::Raw template symbol comes out black, which a \
                 flat dark background would hide completely.",
            ),
            grid(catalogue),
            row("one symbol across sizes", sizes),
            row("one symbol across FontWeight::ALL", weights),
            demo(
                "a name this OS does not have",
                stack(vec![
                    mono(format!(
                        "sf_symbol({ABSENT_SYMBOL:?}, ..) = {}",
                        match sf_symbol(ABSENT_SYMBOL, base, weight) {
                            Some(_) => "Some(..)",
                            None => "None",
                        }
                    )),
                    caption(
                        "None, not a placeholder glyph — a stand-in would hide the difference \
                         between a name macOS 26 has and one an older release does not.",
                    ),
                ]),
            ),
        ],
    )
}

fn symbol_cell(name: &str, size: Pixels, weight: FontWeight, color: &SymbolColor) -> AnyElement {
    let body = match sf_symbol_colored(name, size, weight, color) {
        Some(symbol) => div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(symbol.img())
            .into_any_element(),
        None => caption("None").into_any_element(),
    };
    checker(44., body)
}

fn symbol_color_name(color: &SymbolColor) -> String {
    match color {
        SymbolColor::Label => "Label".into(),
        SymbolColor::Hierarchical(c) => format!("Hierarchical({c:?})"),
        SymbolColor::Palette(colors) => format!("Palette({} colours)", colors.len()),
        SymbolColor::Multicolor => "Multicolor".into(),
        SymbolColor::Raw => "Raw".into(),
    }
}

fn cache_section() -> AnyElement {
    let stats = cache_stats();
    section(
        "cache",
        vec![
            mono(format!(
                "generation() = {}    cache generation = {}    palette_resolves = {}    \
                 symbol_rasters = {}",
                generation(),
                stats.generation,
                stats.palette_resolves,
                stats.symbol_rasters
            )),
            caption(
                "Counters, not gauges. The palette cache is keyed by appearance, so only \
                 re-entering an appearance you have already visited distinguishes a live cache \
                 from a stale one.",
            ),
        ],
    )
}

fn hex8(c: Hsla) -> String {
    let c = Rgba::from(c);
    let q = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", q(c.r), q(c.g), q(c.b), q(c.a))
}

fn opt_hex(c: Option<Hsla>) -> String {
    c.map_or_else(|| "refused".to_string(), hex8)
}
