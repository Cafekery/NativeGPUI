use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{App, AppContext, AsyncApp, Bounds, Render, Window, WindowBounds, point, px, size};
use native_gpui::window_options;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const TRIES: usize = 12;

static FAILED: AtomicBool = AtomicBool::new(false);
static RAN: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);

pub fn report_due(frames: usize) -> bool {
    frames > 3 && !REPORTED.swap(true, Ordering::Relaxed)
}

pub fn fail(why: &str) {
    println!("FAIL: {why}");
    FAILED.store(true, Ordering::Relaxed);
}

pub fn gpui_view(window: &Window) -> Retained<NSView> {
    let handle = HasWindowHandle::window_handle(window).expect("macOS window has an AppKit handle");
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        unreachable!("macOS-only example saw a non-AppKit window handle");
    };
    unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }
        .expect("the AppKit handle always carries a live NSView")
}

pub fn class_of(view: &NSView) -> String {
    let obj: &AnyObject = view.as_ref();
    obj.class().name().to_string_lossy().into_owned()
}

/// By ancestry, not name: AppKit's private hosting view carries the container's name in its own.
fn is_kind(view: &NSView, name: &str) -> bool {
    let obj: &AnyObject = view.as_ref();
    std::iter::successors(Some(obj.class()), |class| class.superclass())
        .any(|class| class.name().to_string_lossy() == name)
}

pub fn find(root: &NSView, name: &str) -> Option<Retained<NSView>> {
    root.subviews().iter().find_map(|sub| {
        if is_kind(&sub, name) {
            Some(sub)
        } else {
            find(&sub, name)
        }
    })
}

fn within(hit: *mut NSView, target: &NSView) -> bool {
    let cursor = unsafe { Retained::retain(hit) };
    std::iter::successors(cursor, |view| unsafe { view.superview() })
        .any(|view| std::ptr::eq(Retained::as_ptr(&view), target))
}

pub fn centre_in(view: &NSView, space: &NSView) -> NSPoint {
    let b = view.bounds();
    view.convertPoint_toView(
        NSPoint::new(
            b.origin.x + b.size.width / 2.,
            b.origin.y + b.size.height / 2.,
        ),
        Some(space),
    )
}

pub fn covers(view: &NSView, space: &NSView, point: NSPoint) -> bool {
    let r = view.convertRect_toView(view.bounds(), Some(space));
    point.x >= r.origin.x
        && point.x <= r.origin.x + r.size.width
        && point.y >= r.origin.y
        && point.y <= r.origin.y + r.size.height
}

pub fn hit_rows(
    gpui: &NSView,
    rows: &[(&str, NSPoint, &NSView, bool)],
    row_w: usize,
    hit_w: usize,
) {
    println!(
        "{:<row_w$} {:<16} {:<hit_w$} VERDICT",
        "ROW", "POINT", "hitTest:"
    );
    for &(what, at, want, exact) in rows {
        // `hitTest:` takes its point in the receiver's superview space.
        let hit: *mut NSView = unsafe { objc2::msg_send![gpui, hitTest: at] };
        let name = if hit.is_null() {
            "nil".to_string()
        } else {
            class_of(unsafe { &*hit })
        };
        let ok = if exact {
            std::ptr::eq(hit.cast_const(), want)
        } else {
            within(hit, want)
        };
        let verdict = match (ok, exact) {
            (true, _) => "ok".to_string(),
            (false, true) => format!("WRONG: wanted exactly {}", class_of(want)),
            (false, false) => format!("WRONG: wanted {}", class_of(want)),
        };
        if !ok {
            FAILED.store(true, Ordering::Relaxed);
        }
        println!(
            "{what:<row_w$} {:<16} {name:<hit_w$} {verdict}",
            format!("{:.0},{:.0}", at.x, at.y),
        );
    }
    RAN.store(true, Ordering::Relaxed);
}

pub fn run<V: Render>(width: f32, height: f32, probe: impl FnOnce() -> V + 'static) -> ExitCode {
    gpui_platform::application().run(move |cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(width), px(height)),
                    })),
                    // gpui starts its display link only once occlusion reports visible.
                    focus: false,
                    ..window_options()
                },
                |_, cx| cx.new(|_| probe()),
            )
            .unwrap();

        cx.spawn(async move |cx: &mut AsyncApp| {
            for _ in 0..TRIES {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                let open = window.update(cx, |_, _, cx| cx.notify()).is_ok();
                if !open || REPORTED.load(Ordering::Relaxed) {
                    break;
                }
            }
            if !RAN.load(Ordering::Relaxed) {
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
