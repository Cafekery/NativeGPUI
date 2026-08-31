//! Whether [`system_font_tabular`] actually produces tabular figures.
//!
//! The token is one line — a `FontFeatures` list carrying `tnum` — and its whole value rests on
//! GPUI's shaper honouring that tag on `.AppleSystemUIFont`. If a toolchain stops honouring it the
//! function still compiles, still returns a `Font`, and returns a proportional one: a caller then
//! has a name promising a stable column and a column that still shifts under every row. Nothing
//! else in the gate would notice, so this exists.
//!
//! It cannot be a unit test. An OpenType feature is a shaping-time glyph substitution, so it is
//! invisible to `TextSystem::advance`, which looks up the character's default glyph and asks its
//! width. Only `shape_line` shapes, and `shape_line` lives on `WindowTextSystem` — so this needs a
//! real window. It takes no keyboard focus and captures no pixels; every number comes out of
//! GPUI's own text system in-process.
//!
//! Three arms, because the interesting one is a *null* — "the digits came out equal" — and a null
//! is worthless from an instrument that cannot say otherwise:
//!
//! * `Menlo` with no features must read TABULAR. It is monospaced on every Mac, so an instrument
//!   that cannot see equal advances there is measuring nothing.
//! * `.AppleSystemUIFont` with no features must read proportional. If the plain system font
//!   already reads tabular there is no change left to detect and every verdict below is vacuous.
//! * A tag no font implements must leave the spread alone. Without it, "tnum equalised the digits"
//!   could just as well mean "attaching any feature perturbs the shaper", which would make the
//!   choice of tag arbitrary and the token a coincidence.
//!
//! The width is also checked against a *value*, not just against itself: AppKit's own
//! `monospacedDigitSystemFont(ofSize: 14, weight: .regular)`, shaped through `CTLine`, puts ten
//! digits at 86.68pt. Equal-but-wrong would pass a spread test on its own.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, Application, AsyncApp, Bounds, Context, Font, FontFeatures, FontWeight, Pixels, Render,
    TextRun, Window, WindowBounds, div, hsla, point, prelude::*, px, size,
};
use native_gpui::tokens::{system_font, system_font_tabular};
use native_gpui::window_options;

/// Ten of one digit, so a per-digit difference of a third of a point is ten times as visible as the
/// f32 noise floor.
const REPEATS: usize = 10;

/// AppKit's `monospacedDigitSystemFont(14, .regular)` through `CTLine`, for [`REPEATS`] digits.
const APPKIT_TABULAR_WIDTH: f32 = 86.68;

/// Below this a spread is equality: the widest and narrowest SF digits differ by 25.16pt over ten
/// repeats, so there is no ambiguous middle to worry about.
const EQUAL: f32 = 0.01;

static FAILED: AtomicU8 = AtomicU8::new(0);
static RAN: AtomicU8 = AtomicU8::new(0);

fn tag(name: &str) -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![(name.to_string(), 1)]))
}

fn shaped_width(window: &Window, font: &Font, size: Pixels, text: &str) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: hsla(0., 0., 0., 1.),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(text.to_string().into(), size, &[run], None)
            .width,
    )
}

/// Widths of `REPEATS` copies of each of `0`..`9`, and how far apart the widest and narrowest are.
fn digit_spread(window: &Window, font: &Font, size: Pixels) -> (Vec<f32>, f32) {
    let widths: Vec<f32> = ('0'..='9')
        .map(|d| {
            let text: String = std::iter::repeat(d).take(REPEATS).collect();
            shaped_width(window, font, size, &text)
        })
        .collect();
    let spread = widths.iter().cloned().fold(f32::MIN, f32::max)
        - widths.iter().cloned().fold(f32::MAX, f32::min);
    (widths, spread)
}

enum Want {
    Tabular,
    Proportional,
}

fn row(window: &Window, label: &str, font: &Font, size: Pixels, want: Want) -> f32 {
    let (widths, spread) = digit_spread(window, font, size);
    RAN.fetch_add(1, Ordering::Relaxed);
    let tabular = spread < EQUAL;
    let ok = match want {
        Want::Tabular => tabular,
        Want::Proportional => !tabular,
    };
    if !ok {
        FAILED.fetch_add(1, Ordering::Relaxed);
    }
    println!(
        "{label:<44} {:>9.2} {:>9.2} {spread:>9.4}  {:<12} {}",
        widths[1],
        widths[4],
        if tabular { "tabular" } else { "proportional" },
        if ok { "ok" } else { "WRONG" }
    );
    spread
}

struct Probe {
    done: bool,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if !self.done {
            self.done = true;
            let size = px(14.);

            println!(
                "{:<44} {:>9} {:>9} {:>9}  {:<12} {}",
                "FONT", "'1'x10", "'4'x10", "SPREAD", "FIGURES", "VERDICT"
            );

            let mut menlo = system_font(FontWeight::NORMAL);
            menlo.family = "Menlo".into();
            row(
                window,
                "Menlo, no features [instrument positive]",
                &menlo,
                size,
                Want::Tabular,
            );
            row(
                window,
                "system_font(NORMAL) [instrument negative]",
                &system_font(FontWeight::NORMAL),
                size,
                Want::Proportional,
            );

            let mut nonsense = system_font(FontWeight::NORMAL);
            nonsense.features = tag("zzzz");
            row(
                window,
                "system face + zzzz=1 [tag is not arbitrary]",
                &nonsense,
                size,
                Want::Proportional,
            );

            let tabular = system_font_tabular(FontWeight::NORMAL);
            row(
                window,
                "system_font_tabular(NORMAL) [the subject]",
                &tabular,
                size,
                Want::Tabular,
            );

            // Equal is not enough: it has to be equal to AppKit's own answer, or the feature could
            // be substituting some other equal-width set.
            //
            // The digit has to be `1`. SF's proportional `0` already advances 8.668pt, which is
            // the tabular advance, so ten `0`s measure 86.68 whether the feature applied or not —
            // a first draft of this check used `0` and stayed green while the token returned a
            // proportional font. `1` is the narrowest at 6.34, so ten of them are 63.44 without
            // the feature and 86.68 with it.
            let width = shaped_width(window, &tabular, size, &"1".repeat(REPEATS));
            let off = (width - APPKIT_TABULAR_WIDTH).abs();
            RAN.fetch_add(1, Ordering::Relaxed);
            let matches = off < 0.05;
            if !matches {
                FAILED.fetch_add(1, Ordering::Relaxed);
            }
            println!(
                "\n{REPEATS} '1's: {width:.2}pt vs AppKit monospacedDigitSystemFont \
                 {APPKIT_TABULAR_WIDTH:.2}pt, off by {off:.4}  {}",
                if matches { "ok" } else { "WRONG" }
            );

            // The jitter a date column would show, in the units the caller cares about.
            for (label, font) in [
                ("system_font", system_font(FontWeight::NORMAL)),
                ("system_font_tabular", tabular.clone()),
            ] {
                let a = shaped_width(window, &font, size, "11:11\u{202F}AM");
                let b = shaped_width(window, &font, size, "10:00\u{202F}AM");
                println!(
                    "{label:<22} \"11:11 AM\" {a:>6.2}   \"10:00 AM\" {b:>6.2}   jitter {:>5.2}pt",
                    (a - b).abs()
                );
            }
            // The one row whose failure is the whole point of the file.
            let a = shaped_width(window, &tabular, size, "11:11\u{202F}AM");
            let b = shaped_width(window, &tabular, size, "10:00\u{202F}AM");
            RAN.fetch_add(1, Ordering::Relaxed);
            if (a - b).abs() >= EQUAL {
                FAILED.fetch_add(1, Ordering::Relaxed);
                println!(
                    "WRONG: a tabular column still shifts by {:.2}pt",
                    (a - b).abs()
                );
            }
        }
        div().size_full()
    }
}

fn main() -> ExitCode {
    Application::new().run(|cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(8.), px(8.)),
                        size: size(px(160.), px(90.)),
                    })),
                    ..window_options()
                },
                |_, cx| cx.new(|_| Probe { done: false }),
            )
            .expect("a window is required: shape_line lives on WindowTextSystem");

        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..12 {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                let done = window
                    .update(cx, |probe, _, cx| {
                        cx.notify();
                        probe.done
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            // `Application::run` returning exits 0 regardless, so the verdict has to leave here.
            // Fail closed: the loop also ends by running out of tries or by `window.update`
            // erroring, and neither of those has measured anything.
            if RAN.load(Ordering::Relaxed) == 0 {
                println!("FAIL: nothing was measured — the window never rendered");
                std::process::exit(1);
            }
            std::process::exit(FAILED.load(Ordering::Relaxed) as i32);
        })
        .detach();
    });
    ExitCode::SUCCESS
}
