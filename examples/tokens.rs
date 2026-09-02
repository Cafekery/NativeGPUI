//! `native_gpui::tokens` on screen: colours, fonts, symbols, and a live appearance switch.

use std::ptr;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Hsla, MouseButton, Render, RenderImage, Rgba, Window, WindowBounds,
    WindowOptions, div, img, point, prelude::*, px, size,
};
use image::{Frame, RgbaImage};
use native_gpui::tokens::{
    self, Appearance, SymbolColor, SystemColor, SystemTheme, cache_stats, current_appearance,
    sf_symbol_colored, small_system_font_size, system_font, system_font_family,
    system_font_metrics, system_font_size, text_font_family,
};
use native_gpui::{ControlSize, native_root, native_switch};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing, NSBitmapFormat, NSBitmapImageRep, NSColor,
    NSDeviceRGBColorSpace, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSGraphicsContext, NSStringDrawing,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSPoint, NSSize, NSString};

const SAMPLE: &str = "Handgloves 0123 xnxn";
const SWATCH_COLS: usize = 6;

fn hex8(c: Hsla) -> String {
    let c = Rgba::from(c);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", q(c.r), q(c.g), q(c.b), q(c.a))
}

/// AppKit hands back premultiplied RGBA; gpui's `RenderImage` wants straight-alpha BGRA.
fn to_straight_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

struct TextStrip {
    size: f32,
    image: Arc<RenderImage>,
    w: f32,
    h: f32,
}

fn appkit_text_strip(text: &str, point_size: f64, color: &NSColor, scale: f64) -> TextStrip {
    let font = NSFont::systemFontOfSize(point_size);
    let font_obj: &AnyObject = font.as_ref();
    let color_obj: &AnyObject = color.as_ref();
    let attrs: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
        &[unsafe { NSFontAttributeName }, unsafe {
            NSForegroundColorAttributeName
        }],
        &[font_obj, color_obj],
    );

    let ns_text = NSString::from_str(text);
    let measured = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &ns_text,
            Some(&attrs),
        )
    }
    .size();

    let w = (measured.width.ceil() as usize).max(1);
    let h = (measured.height.ceil() as usize).max(1);
    let pw = ((w as f64 * scale).round() as usize).max(1);
    let ph = ((h as f64 * scale).round() as usize).max(1);
    let points = NSSize::new(w as f64, h as f64);

    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            pw as isize,
            ph as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            (pw * 4) as isize,
            32,
        )
    }
    .expect("bitmap rep");
    // Size in points, buffer in pixels: that mismatch is what applies the 2x transform.
    rep.setSize(points);

    let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).expect("context");
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&ctx));
    unsafe { ns_text.drawAtPoint_withAttributes(NSPoint::new(0.0, 0.0), Some(&attrs)) };
    ctx.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let src = rep.bitmapData();
    let stride = rep.bytesPerRow() as usize;
    let mut rgba = vec![0u8; pw * ph * 4];
    if !src.is_null() {
        for y in 0..ph {
            unsafe {
                ptr::copy_nonoverlapping(
                    src.add(y * stride),
                    rgba.as_mut_ptr().add(y * pw * 4),
                    pw * 4,
                );
            }
        }
    }

    TextStrip {
        size: point_size as f32,
        image: Arc::new(RenderImage::new(vec![Frame::new(
            RgbaImage::from_raw(pw as u32, ph as u32, to_straight_bgra(&rgba))
                .expect("strip buffer is w*h*4"),
        )])),
        w: w as f32,
        h: h as f32,
    }
}

const SYMBOLS: &[(&str, fn() -> SymbolColor)] = &[
    ("gearshape", || {
        SymbolColor::Hierarchical(SystemColor::SystemBlue)
    }),
    ("folder.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemTeal)
    }),
    ("bolt.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemOrange)
    }),
    ("checkmark.seal.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemGreen)
    }),
    ("bell.badge", || {
        SymbolColor::Palette(vec![SystemColor::SystemRed, SystemColor::SystemGray])
    }),
    ("star.fill", || {
        SymbolColor::Hierarchical(SystemColor::SystemYellow)
    }),
    ("trash", || SymbolColor::Label),
    ("magnifyingglass", || SymbolColor::Label),
    ("wifi", || SymbolColor::Label),
    ("paintpalette.fill", || SymbolColor::Multicolor),
];

struct Tokens {
    printed: bool,
    dark: bool,
    strips: Vec<TextStrip>,
}

impl Tokens {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
            .detach();
        Self {
            printed: false,
            dark: current_appearance() == Appearance::Dark,
            strips: [11.0f64, 13.0, 17.0]
                .map(|size| appkit_text_strip(SAMPLE, size, &NSColor::labelColor(), 2.0))
                .into(),
        }
    }

    fn print_summary(&self, cx: &App) {
        println!(
            "appearance={:?}  accent={}",
            current_appearance(),
            hex8(tokens::accent())
        );
        println!(
            "refused sRGB conversion: light={:?} dark={:?}",
            tokens::refused(Appearance::Light),
            tokens::refused(Appearance::Dark)
        );

        let text_family = text_font_family(cx);
        println!(
            "\nsystem_font_family()={}  text_font_family()={}",
            system_font_family(),
            text_family.unwrap_or("None (SF Pro not installed)")
        );
        println!(
            "systemFontSize={:.0}pt  smallSystemFontSize={:.0}pt  labelFontSize={:.0}pt",
            f32::from(system_font_size()),
            f32::from(small_system_font_size()),
            f32::from(tokens::label_font_size())
        );
        println!(
            "system_font_size_for: {}",
            [
                ("Mini", ControlSize::Mini),
                ("Small", ControlSize::Small),
                ("Regular", ControlSize::Regular),
                ("Large", ControlSize::Large),
                ("ExtraLarge", ControlSize::ExtraLarge),
            ]
            .map(|(n, s)| format!("{n}={:.0}pt", f32::from(tokens::system_font_size_for(s))))
            .join("  ")
        );
        let m = system_font_metrics(system_font_size(), gpui::FontWeight::NORMAL);
        println!(
            "metrics at {:.0}pt: asc={:+.3} desc={:+.3} lead={:.3} cap={:.3} x={:.3}  face={}",
            f32::from(system_font_size()),
            m.ascent,
            m.descent,
            m.leading,
            m.cap_height,
            m.x_height,
            m.resolved_name
        );

        let w = gpui::FontWeight::NORMAL;
        println!("\nadvance of 'x', GPUI minus AppKit, in points:");
        println!(
            "  {:>5} {:>11} {:>10} {:>10} {:>12}",
            "size", "AppKit", ".AppleSys", "Menlo", "HelvNeue"
        );
        for size in [9.0f32, 11.0, 13.0, 16.0, 17.0, 24.0] {
            let at = px(size);
            let delta = |family| {
                tokens::advance_delta(cx, family, at, w, 'x')
                    .map(|d| format!("{d:+.4}"))
                    .unwrap_or_else(|| "n/a".into())
            };
            println!(
                "  {size:>5.0} {:>11.4} {:>10} {:>10} {:>12}",
                tokens::appkit_advance(system_font_family(), at, w, 'x').unwrap_or(f32::NAN),
                delta(system_font_family()),
                delta("Menlo"),
                delta("Helvetica Neue"),
            );
        }

        println!("\ncumulative drift over {SAMPLE:?}:");
        let ts = cx.text_system();
        let mut font = system_font(w);
        for size in [11.0f32, 13.0, 17.0] {
            let appkit =
                tokens::appkit_string_width(system_font_family(), px(size), w, SAMPLE).unwrap();
            font.family = system_font_family().into();
            let id = ts.resolve_font(&font);
            let gpui: f32 = SAMPLE
                .chars()
                .filter_map(|ch| ts.advance(id, px(size), ch).ok())
                .map(|a| f32::from(a.width))
                .sum();
            println!(
                "  {size:>4.0}pt  AppKit={appkit:>8.3}pt  GPUI={gpui:>8.3}px  {:+.3}pt ({:+.2}%)",
                gpui - appkit,
                100.0 * (gpui - appkit) / appkit
            );
        }

        let missing: Vec<&str> = SYMBOLS
            .iter()
            .filter(|(name, color)| {
                sf_symbol_colored(name, system_font_size(), w, &color()).is_none()
            })
            .map(|(name, _)| *name)
            .collect();
        println!("\nsymbols missing on this OS: {missing:?}");
    }

    fn swatch_grid(&self) -> impl IntoElement {
        let all = SystemColor::ALL;
        let rows = all.len().div_ceil(SWATCH_COLS);
        let mut grid = div().flex().flex_row().gap(px(4.));
        for col in 0..SWATCH_COLS {
            let mut column = div().flex().flex_col().w(px(130.)).overflow_hidden();
            for row in 0..rows {
                let Some(c) = all.get(col * rows + row) else {
                    break;
                };
                column = column.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(3.))
                        .h(px(13.))
                        .child(
                            // `w()` is a flex basis, not a floor: a long label squeezes the swatch.
                            div()
                                .flex_shrink_0()
                                .w(px(24.))
                                .h(px(11.))
                                .bg(tokens::system_color(*c))
                                .border_1()
                                .border_color(tokens::system_color(SystemColor::Separator)),
                        )
                        .child(
                            div()
                                .text_size(px(8.))
                                .text_color(tokens::system_color(SystemColor::SecondaryLabel))
                                .child(c.objc_name().trim_end_matches("Color").to_string()),
                        ),
                );
            }
            grid = grid.child(column);
        }
        grid
    }

    fn font_rows(&self) -> impl IntoElement {
        let mut col = div().flex().flex_col().gap(px(2.));
        for strip in &self.strips {
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(26.))
                            .text_size(px(8.))
                            .text_color(tokens::system_color(SystemColor::TertiaryLabel))
                            .child(format!("{:.0}pt", strip.size)),
                    )
                    .child(
                        div()
                            .w(px(230.))
                            .font(system_font(gpui::FontWeight::NORMAL))
                            .text_size(px(strip.size))
                            .text_color(tokens::system_color(SystemColor::Label))
                            .child(SAMPLE),
                    )
                    .child(img(strip.image.clone()).w(px(strip.w)).h(px(strip.h)))
                    .child(
                        div()
                            .text_size(px(8.))
                            .text_color(tokens::system_color(SystemColor::TertiaryLabel))
                            .child("< GPUI | AppKit >"),
                    ),
            );
        }
        col
    }

    /// A `Symbol` held across an appearance change is a stale raster, so ask for one per render.
    fn symbol_row(&self) -> impl IntoElement {
        let size = system_font_size();
        let mut row = div().flex().flex_row().flex_wrap().gap(px(6.));
        for (name, color) in SYMBOLS {
            let mut cell = div().flex().flex_row().items_center().gap(px(3.));
            if let Some(s) = sf_symbol_colored(name, size, gpui::FontWeight::NORMAL, &color()) {
                cell = cell.child(s.img());
            }
            row = row.child(
                cell.child(
                    div()
                        .font(system_font(gpui::FontWeight::NORMAL))
                        .text_size(size)
                        .text_color(tokens::system_color(SystemColor::Label))
                        .child(name.to_string()),
                ),
            );
        }
        row
    }
}

impl Render for Tokens {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.printed {
            self.printed = true;
            self.print_summary(cx);
        }
        let theme = *cx.global::<SystemTheme>();
        let stats = cache_stats();
        let secondary = tokens::system_color(SystemColor::SecondaryLabel);

        native_root(
            div()
                .size_full()
                .bg(tokens::system_color(SystemColor::WindowBackground))
                .font(system_font(gpui::FontWeight::NORMAL))
                .text_size(system_font_size())
                .text_color(tokens::system_color(SystemColor::Label))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .pt(px(24.))
                        .px(px(8.))
                        .pb(px(8.))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.))
                                .child(div().text_size(px(9.)).text_color(secondary).child(
                                    format!(
                                        "appearance={:?}  accent={}  generation={}  palette_resolves={}  symbol_rasters={}",
                                        theme.appearance,
                                        hex8(theme.accent),
                                        theme.generation,
                                        stats.palette_resolves,
                                        stats.symbol_rasters
                                    ),
                                ))
                                .child(native_switch("appearance", self.dark).on_change(
                                    cx.listener(|this, on: &bool, _window, cx| {
                                        this.dark = *on;
                                        tokens::set_app_appearance(Some(if *on {
                                            Appearance::Dark
                                        } else {
                                            Appearance::Light
                                        }));
                                        cx.notify();
                                    }),
                                ))
                                .child(div().text_size(px(9.)).text_color(secondary).child("dark"))
                                .child(
                                    div()
                                        .id("follow-system")
                                        .px(px(5.))
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(tokens::system_color(SystemColor::ControlAccent))
                                        .text_size(px(9.))
                                        .text_color(tokens::system_color(
                                            SystemColor::AlternateSelectedControlText,
                                        ))
                                        .child("follow system")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                tokens::set_app_appearance(None);
                                                this.dark =
                                                    current_appearance() == Appearance::Dark;
                                                cx.notify();
                                            }),
                                        ),
                                ),
                        )
                        .child(self.swatch_grid())
                        .child(self.font_rows())
                        .child(self.symbol_row()),
                ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(1400.), px(880.)),
                    size: size(px(820.), px(400.)),
                })),
                ..native_gpui::window_options()
            },
            |_window, cx| cx.new(Tokens::new),
        )
        .unwrap();
        cx.activate(true);
    });
}
