//! Gate: a `glass_container` declines mouse points that belong to something else, and a glass
//! member still claims its own. Exits 0 pass, 1 fail.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, AsyncApp, Bounds, Context, Render, Window, WindowBounds, div, point, prelude::*, px, size,
};
use native_gpui::{glass, glass_container, native_root, native_switch, window_options};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

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
                .child(div().absolute().left(px(20.)).top(px(20.)).child(
                    native_switch("knob", on).on_change(
                        cx.listener(|probe: &mut Probe, checked: &bool, _, _| probe.on = *checked),
                    ),
                ))
                .child(
                    glass_container("batch").child(
                        div()
                            .absolute()
                            .left(px(150.))
                            .top(px(20.))
                            .w(px(90.))
                            .h(px(40.))
                            .child(glass("panel")),
                    ),
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

/// By ancestry, not name: AppKit's private hosting view carries the container's name in its own.
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
    // Private glass subviews have zero frames, and decline every point, until laid out.
    content.layoutSubtreeIfNeeded();
    content.displayIfNeeded();

    let (Some(knob), Some(panel), Some(batch)) = (
        find(&content, "NSSwitch"),
        find(&content, "NSGlassEffectView"),
        find(&content, "NSGlassEffectContainerView"),
    ) else {
        fail("the library placed no switch, no glass panel, or no glass container");
        return;
    };
    println!("container class: {}", class_of(&batch));

    let knob_at = centre_in(&knob, &content);
    let panel_at = centre_in(&panel, &content);
    let batch_bounds = batch.bounds();
    if batch_bounds.size.width < 100. || batch_bounds.size.height < 100. {
        fail("the glass container is too small for its own empty area to be probed");
        return;
    }
    let empty_at = batch.convertPoint_toView(
        NSPoint::new(
            batch_bounds.origin.x + batch_bounds.size.width - 20.,
            batch_bounds.origin.y + batch_bounds.size.height - 20.,
        ),
        Some(&content),
    );

    // The container is unflipped between flipped views: a missed conversion mirrors y.
    let mirror_at = NSPoint::new(panel_at.x, content.bounds().size.height - panel_at.y);
    if covers(&panel, &content, mirror_at) || covers(&knob, &content, mirror_at) {
        fail("the mirrored member point lands on a hosted view; move the member or the window");
        return;
    }

    println!("{:<40} {:<16} {:<34} VERDICT", "ROW", "POINT", "hitTest:");
    let rows: [(&str, NSPoint, &NSView, bool); 4] = [
        (
            "over the switch, below the container",
            knob_at,
            &knob,
            false,
        ),
        ("over a glass member", panel_at, &panel, false),
        ("the member's point, y mirrored", mirror_at, &gpui, true),
        ("over bare GPUI inside the container", empty_at, &gpui, true),
    ];
    for (what, at, want, exact) in rows {
        // `hitTest:` takes its point in the receiver's superview space.
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
            "{:<40} {:<16} {:<34} {verdict}",
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
                        size: size(px(260.), px(200.)),
                    })),
                    // gpui starts its display link only once occlusion reports visible.
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
