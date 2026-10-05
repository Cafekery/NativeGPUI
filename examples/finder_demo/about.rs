use std::cell::RefCell;

use gpui::{
    App, Bounds, Context, Div, FontWeight, Pixels, Window, WindowBounds, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, FileType, SystemColor, system_color};
use native_gpui::{
    LabelAlign, native_label, native_raster, native_root, titlebar_band, window_options,
};
use objc2::MainThreadMarker;
use objc2_foundation::{NSBundle, NSString};

use crate::chrome::toolbar::{system_corners, titled_height};

const WIDTH: f32 = 301.;
const HEIGHT: f32 = 300.;
const ICON: f32 = 128.;
const ICON_X: f32 = 86.;
const ICON_Y: f32 = 22.;
const TEXT_X: f32 = 17.;

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<About>>> = const { RefCell::new(None) };
}

pub(crate) fn show(cx: &mut App) {
    let raised = OPEN.with_borrow(|open| *open).is_some_and(|open| {
        open.update(cx, |_, window, _| window.activate_window())
            .is_ok()
    });
    if raised {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let mut bounds = Bounds::centered(None, size(px(WIDTH), titled_height(HEIGHT, mtm)), cx);
    // AppKit widens a frame whose origin is off whole points.
    bounds.origin = point(bounds.origin.x.floor(), bounds.origin.y.floor());
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            is_resizable: false,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title("");
            system_corners(window);
            cx.new(|cx| {
                cx.on_release(|_, _| OPEN.with_borrow_mut(|open| *open = None))
                    .detach();
                About
            })
        },
    );
    if let Ok(opened) = opened {
        OPEN.with_borrow_mut(|open| *open = Some(opened));
    }
}

fn bundle_string(key: &str) -> String {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str(key))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn line(id: &'static str, top: f32, text: &str, size: Pixels, weight: FontWeight) -> Div {
    div()
        .absolute()
        .left(px(TEXT_X))
        .right(px(TEXT_X))
        .top(px(top))
        .child(
            native_label(id, text)
                .font_size(size)
                .weight(weight)
                .color(SystemColor::ControlText)
                .align(LabelAlign::Center),
        )
}

struct About;

impl Render for About {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let name = bundle_string("CFBundleName");
        let (regular, small) = (tokens::system_font_size(), tokens::small_system_font_size());
        let icon = tokens::file_icon(FileType::Named("NSApplicationIcon"), px(ICON));
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .when_some(icon, |about, icon| {
                            about.child(
                                div()
                                    .absolute()
                                    .left(px(ICON_X))
                                    .top(px(ICON_Y))
                                    .child(native_raster("ng.finder.about.icon", icon)),
                            )
                        })
                        .child(line(
                            "ng.finder.about.name",
                            156.,
                            &name,
                            regular,
                            FontWeight::BOLD,
                        ))
                        .child(line(
                            "ng.finder.about.tagline",
                            180.,
                            "Real AppKit views in a GPUI window",
                            regular,
                            FontWeight::NORMAL,
                        ))
                        .child(line(
                            "ng.finder.about.version",
                            207.,
                            &format!(
                                "{name} version {}",
                                bundle_string("CFBundleShortVersionString")
                            ),
                            small,
                            FontWeight::NORMAL,
                        ))
                        .child(line(
                            "ng.finder.about.build",
                            224.,
                            &format!("Build {}", bundle_string("CFBundleVersion")),
                            small,
                            FontWeight::NORMAL,
                        ))
                        .child(line(
                            "ng.finder.about.copyright",
                            247.,
                            "\u{a9} 2026 Jace",
                            small,
                            FontWeight::NORMAL,
                        ))
                        .child(line(
                            "ng.finder.about.license",
                            263.,
                            "Released under the MIT License.",
                            small,
                            FontWeight::NORMAL,
                        )),
                ),
        )
    }
}
