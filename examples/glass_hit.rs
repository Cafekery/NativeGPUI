//! Whether a `glass_container` claims mouse points that belong to something else.
//!
//! `NSGlassEffectContainerView` answers `hitTest:` with **self** for every point inside its
//! bounds, and the container spans the whole window, so unless it is subclassed one
//! [`glass_container`] makes every hosted control painted before it unclickable. Both directions
//! matter: a container that declined *everything* would revive the control and kill the glass,
//! and would look identical to a fix if only the control were checked.
//!
//! Every verdict is `hitTest:` asked of AppKit in-process, on views the library itself placed, so
//! nothing here needs a screenshot or the keyboard. Exits non-zero naming each wrong row.
//!
//! It cannot be a unit test: libtest runs every test body on a spawned thread, where
//! `MainThreadMarker::new()` is `None` and no `NSView` can honestly be built.
//!
//! Two traps this is built around, both of which would make it pass by measuring nothing:
//!
//! * The probe points are the centres of the views AppKit actually placed, converted out of those
//!   views, not coordinates written here. A hardcoded point that missed would report the same
//!   "nothing claimed it" as a genuinely dead control.
//! * The private subviews inside both glass classes have zero frames until something lays them
//!   out, and a zero-framed view declines every point — indistinguishable from declining on
//!   purpose. So this needs a *rendered* window, which is laid out by construction, plus
//!   `layoutSubtreeIfNeeded` before probing, plus a row that requires a CLAIM. Rewriting this as
//!   a faster windowless harness reads every private subview as declining and every row goes
//!   quietly green.

use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    App, Application, AsyncApp, Bounds, Context, Render, Window, WindowBounds, div, point,
    prelude::*, px, size,
};
use native_gpui::{glass, glass_container, native_root, native_switch, window_options};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

struct Probe {
    /// Frames painted. The tree has to settle and `commit` has to have run.
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
                // Painted first, so `commit` stacks its slot BELOW the container's — the
                // arrangement that used to kill it.
                .child(
                    div().absolute().left(px(20.)).top(px(20.)).child(
                        native_switch("knob", on).on_change(cx.listener(
                            |probe: &mut Probe, checked: &bool, _, _| probe.on = *checked,
                        )),
                    ),
                )
                .child(
                    // Near the top, so the vertical mirror of its centre lands in bare GPUI —
                    // which is what makes the mirrored row below able to fail.
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

/// Set once every row has been evaluated.
///
/// The frames that make the readings honest are driven by a timer, so "never got four frames"
/// and "all four rows passed" both leave `FAILED` at zero. Without this the whole check exits 0
/// having asserted nothing, on exactly the machines where a window is slowest to render.
static RAN: AtomicU8 = AtomicU8::new(0);

fn fail(why: &str) {
    println!("FAIL: {why}");
    FAILED.store(1, Ordering::Relaxed);
}

/// GPUI's own view. Its superview is the window content view, which is the space every hosted
/// clip frame is written in — so that is the space `hitTest:` on this view wants.
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

/// Whether `view`'s class is `name` or descends from it.
///
/// Not a substring test on the class name. AppKit builds a private
/// `_NSCoreHostingView<NSGlassEffectContainerView.RootView>` inside the container, whose *name*
/// contains the container's — so a substring search finds that instead, and every later reading
/// silently describes the wrong view. Ancestry also gets `NSKVONotifying_NSSwitch` right, which
/// an exact-name test would miss.
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

/// The first view in `root`'s subtree that is a kind of `name`, depth first.
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

/// Whether `hit` is `target` or lives inside it. Answers "did the click land on this control"
/// exactly, without depending on which private subview a class happens to hand back.
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

/// The centre of `view` in `space`'s coordinates.
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

/// Whether `point` falls inside `view`'s rect expressed in `space`.
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
    // Without this the private subviews of both glass classes are still zero-sized, and the glass
    // row would pass by finding nothing.
    content.layoutSubtreeIfNeeded();
    content.displayIfNeeded();

    let (Some(knob), Some(panel), Some(batch)) = (
        find(&content, "NSSwitch"),
        find(&content, "NSGlassEffectView"),
        find(&content, "NSGlassEffectContainerView"),
    ) else {
        // Finding nothing must not read as three quiet passes.
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
    // Inside the container, over neither. The container spans the viewport, so its own far corner
    // is past everything hosted.
    let empty_at = batch.convertPoint_toView(
        NSPoint::new(
            batch_bounds.origin.x + batch_bounds.size.width - 20.,
            batch_bounds.origin.y + batch_bounds.size.height - 20.,
        ),
        Some(&content),
    );

    // The member's own point reflected about the content view's horizontal midline. The container
    // is unflipped and both the clip above it and its `contentView` below it are flipped, so the
    // conversion in `hitTest:` mirrors y twice and cancels. Drop the conversion and the mirrored
    // point is the one that hits the member — while the real point misses. Nothing else in this
    // run can tell those apart: a switch-reachable check passes either way.
    let mirror_at = NSPoint::new(panel_at.x, content.bounds().size.height - panel_at.y);
    if covers(&panel, &content, mirror_at) || covers(&knob, &content, mirror_at) {
        // If the mirror lands back on something hosted the row cannot fail, so say so instead of
        // reporting a pass.
        fail("the mirrored member point lands on a hosted view; move the member or the window");
        return;
    }

    println!(
        "{:<40} {:<16} {:<34} {}",
        "ROW", "POINT", "hitTest:", "VERDICT"
    );
    // `exact` distinguishes "the click reached this view" from "the click landed somewhere inside
    // it". Containment is right for the glass, which hands back a private subview of itself —
    // and wrong for GPUI, whose view is the ancestor of every hosted view there is, so a
    // containment test on that row would pass no matter who claimed the point.
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
        // Asked of GPUI's view, not the content view: `hitTest:` takes its point in the
        // receiver's *superview* space, and the content view's superview is the window's frame
        // view, whose origin differs by the titlebar inset.
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
    Application::new().run(|cx: &mut App| {
        let window = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(260.), px(200.)),
                    })),
                    // gpui starts its display link only once occlusion reports visible, so
                    // `show: false` never renders a frame. Hence a small corner window and
                    // `focus: false`: the smallest thing that gets frames, taking the keyboard
                    // away from nothing.
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
            // Fail closed: the loop also ends by running out of tries, or by `window.update`
            // erroring, and neither of those has tested anything.
            if RAN.load(Ordering::Relaxed) == 0 {
                println!(
                    "FAIL: no row was evaluated — the window rendered fewer than 4 frames within \
                     {} tries, or the probe bailed out above",
                    12
                );
                std::process::exit(1);
            }
            std::process::exit(FAILED.load(Ordering::Relaxed) as i32);
        })
        .detach();
    });
    ExitCode::SUCCESS
}
