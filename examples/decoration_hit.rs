//! Gate: the hosted decorations `material`, `native_label` and `native_rule` decline mouse points
//! that belong to GPUI, while a switch under them still claims its own. Exits 0 pass, 1 fail.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, size,
};
use native_gpui::tokens::SystemColor;
use native_gpui::{
    Blending, Layer, Material, material, native_label, native_root, native_rule, native_switch,
    window_options,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const BAND_H: f32 = 60.;
const TRIES: usize = 12;

struct Probe {
    frames: usize,
    on: bool,
    done: bool,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames += 1;
        if self.frames > 3 && !self.done {
            self.done = true;
            report(window);
        }
        let on = self.on;
        native_root(
            div()
                .size_full()
                .relative()
                .child(div().absolute().left(px(16.)).top(px(16.)).child(
                    native_switch("knob", on).on_change(
                        cx.listener(|probe: &mut Probe, checked: &bool, _, _| probe.on = *checked),
                    ),
                ))
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(0.))
                        .w_full()
                        .h(px(BAND_H))
                        .child(
                            material("band", Material::Header)
                                .layer(Layer::Above)
                                .blending(Blending::WithinWindow)
                                .size_full(),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(120.))
                        .top(px(20.))
                        .w(px(110.))
                        .h(px(18.))
                        .child(native_label("title", "Date Modified")),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(96.))
                        .top(px(14.))
                        .w(px(1.))
                        .h(px(30.))
                        .child(native_rule("divider").color(SystemColor::Separator)),
                ),
        )
    }
}

static FAILED: AtomicU8 = AtomicU8::new(0);
static RAN: AtomicU8 = AtomicU8::new(0);

fn fail(why: &str) {
    println!("FAIL: {why}");
    FAILED.store(1, Ordering::Relaxed);
}

fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("macOS window has an AppKit handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("the AppKit handle always carries a live NSView")
}

fn class_of(view: &NSView) -> String {
    let obj: &AnyObject = view.as_ref();
    obj.class().name().to_string_lossy().into_owned()
}

fn is_kind(view: &NSView, name: &str) -> bool {
    let obj: &AnyObject = view.as_ref();
    let mut class = Some(obj.class());
    while let Some(current) = class {
        if current.name().to_string_lossy() == name {
            return true;
        }
        class = current.superclass();
    }
    false
}

fn find(root: &NSView, name: &str) -> Option<Retained<NSView>> {
    let subviews = root.subviews();
    for i in 0..subviews.count() {
        let sub = subviews.objectAtIndex(i);
        if is_kind(&sub, name) {
            return Some(sub);
        }
        if let Some(found) = find(&sub, name) {
            return Some(found);
        }
    }
    None
}

fn within(hit: *mut NSView, target: &NSView) -> bool {
    let target: *const NSView = target;
    let Some(mut cursor) = (unsafe { Retained::retain(hit) }) else {
        return false;
    };
    loop {
        if std::ptr::eq(Retained::as_ptr(&cursor), target) {
            return true;
        }
        match unsafe { cursor.superview() } {
            Some(parent) => cursor = parent,
            None => return false,
        }
    }
}

fn centre_in(view: &NSView, space: &NSView) -> NSPoint {
    let b = view.bounds();
    view.convertPoint_toView(
        NSPoint::new(
            b.origin.x + b.size.width / 2.,
            b.origin.y + b.size.height / 2.,
        ),
        Some(space),
    )
}

fn covers(view: &NSView, space: &NSView, point: NSPoint) -> bool {
    let r = view.convertRect_toView(view.bounds(), Some(space));
    point.x >= r.origin.x
        && point.x <= r.origin.x + r.size.width
        && point.y >= r.origin.y
        && point.y <= r.origin.y + r.size.height
}

fn report(window: &Window) {
    let gpui = gpui_view(window);
    let content = unsafe { gpui.superview() }.expect("gpui's view is inside a content view");
    content.layoutSubtreeIfNeeded();
    content.displayIfNeeded();

    let (Some(knob), Some(band), Some(label), Some(rule)) = (
        find(&content, "NSSwitch"),
        find(&content, "NSVisualEffectView"),
        find(&content, "NSTextField"),
        find(&content, "NSBox"),
    ) else {
        fail("the library placed no switch, no material, no label or no rule");
        return;
    };
    println!(
        "classes: material {}  label {}  rule {}",
        class_of(&band),
        class_of(&label),
        class_of(&rule)
    );

    let knob_at = centre_in(&knob, &content);
    let label_at = centre_in(&label, &content);
    let rule_at = centre_in(&rule, &content);
    let band_rect = band.convertRect_toView(band.bounds(), Some(&content));
    let bare_at = NSPoint::new(
        band_rect.origin.x + band_rect.size.width - 12.,
        band_rect.origin.y + band_rect.size.height / 2.,
    );

    for (what, at) in [
        ("the switch", knob_at),
        ("the label", label_at),
        ("the rule", rule_at),
        ("the bare band point", bare_at),
    ] {
        if !covers(&band, &content, at) {
            fail(&format!(
                "{what} at {:.0},{:.0} is outside the material, so its row cannot fail",
                at.x, at.y
            ));
            return;
        }
    }
    if covers(&label, &content, bare_at) || covers(&rule, &content, bare_at) {
        fail("the bare band point lands on the label or the rule");
        return;
    }
    if covers(&label, &content, knob_at) || covers(&rule, &content, knob_at) {
        fail("the switch is under the label or the rule, so its row tests the wrong view");
        return;
    }

    println!(
        "{:<44} {:<16} {:<26} {}",
        "ROW", "POINT", "hitTest:", "VERDICT"
    );
    let rows: [(&str, NSPoint, &NSView, bool); 4] = [
        ("over the switch, under the material", knob_at, &knob, false),
        ("over a hosted label", label_at, &gpui, true),
        ("over a hosted rule", rule_at, &gpui, true),
        ("over the material, on nothing else", bare_at, &gpui, true),
    ];
    for (what, at, want, exact) in rows {
        // `hitTest:` takes its point in the receiver's superview space, so this is asked of GPUI's
        // view rather than of the content view, whose superview is the window frame view.
        let hit: *mut NSView = unsafe { objc2::msg_send![&*gpui, hitTest: at] };
        let name = if hit.is_null() {
            "nil".to_string()
        } else {
            class_of(unsafe { &*hit })
        };
        let ok = if exact {
            std::ptr::eq(hit.cast_const(), want as *const NSView)
        } else {
            within(hit, want)
        };
        if !ok {
            FAILED.store(1, Ordering::Relaxed);
        }
        let verdict = if ok {
            "ok".to_string()
        } else if exact {
            format!("WRONG: wanted exactly {}", class_of(want))
        } else {
            format!("WRONG: wanted {}", class_of(want))
        };
        println!(
            "{:<44} {:<16} {:<26} {verdict}",
            what,
            format!("{:.0},{:.0}", at.x, at.y),
            name,
        );
    }
    RAN.store(1, Ordering::Relaxed);
}

fn main() -> ExitCode {
    gpui_platform::application().run(|cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(280.), px(160.)),
                    })),
                    // gpui starts its display link only once occlusion reports visible, so
                    // `show: false` never renders a frame.
                    focus: false,
                    ..window_options()
                },
                |_, cx| {
                    cx.new(|_| Probe {
                        frames: 0,
                        on: false,
                        done: false,
                    })
                },
            )
            .unwrap();

        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..TRIES {
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
                println!(
                    "FAIL: no row was evaluated — the window rendered fewer than 4 frames within \
                     {TRIES} tries, or the probe bailed out above"
                );
                std::process::exit(1);
            }
            std::process::exit(FAILED.load(Ordering::Relaxed) as i32);
        })
        .detach();
    });
    ExitCode::SUCCESS
}
