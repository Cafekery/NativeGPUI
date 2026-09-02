//! Gate: `system_font_tabular` shapes digits to equal advances. Exits 0 pass, non-zero fail.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, Font, FontFeatures, FontWeight, Pixels, Render, TextRun,
    Window, WindowBounds, div, hsla, point, prelude::*, px, size,
};
use native_gpui::tokens::{system_font, system_font_tabular};
use native_gpui::window_options;

const REPEATS: usize = 10;
const APPKIT_TABULAR_WIDTH: f32 = 86.68;
const EQUAL: f32 = 0.01;

static FAILED: AtomicU8 = AtomicU8::new(0);
static RAN: AtomicU8 = AtomicU8::new(0);

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

enum Want {
    Tabular,
    Proportional,
}

fn row(window: &Window, label: &str, font: &Font, size: Pixels, want: Want) {
    let widths: Vec<f32> = ('0'..='9')
        .map(|d| {
            let text: String = std::iter::repeat(d).take(REPEATS).collect();
            shaped_width(window, font, size, &text)
        })
        .collect();
    let spread = widths.iter().cloned().fold(f32::MIN, f32::max)
        - widths.iter().cloned().fold(f32::MAX, f32::min);
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
            nonsense.features = FontFeatures(std::sync::Arc::new(vec![("zzzz".to_string(), 1)]));
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

            // Must be `1`: SF's proportional `0` already advances at the tabular width, so ten of
            // them measure 86.68 whether the feature applied or not.
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
    gpui_platform::application().run(|cx: &mut App| {
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
