//! Measurement apparatus for the native sidebar's pane geometry. `sidebar.rs` is the demo.
//!
//! Every arm verifies the same invariant: gpui reads its logical size from `contentView.frame()`,
//! sizes its drawable from its own frame, and takes mouse coordinates from `locationInWindow`
//! with no view conversion, so GPUI's view is given the contentView's size at a pane-cancelling
//! negative origin. `NG_ARM=fill` is the deliberately-broken control.
//!
//! Env-gated arms:
//!
//! ```text
//! NG_ARM=aligned|fill    pane framing; `fill` is the broken control arm
//! NG_OPAQUE=1            default gpui window background, to show the material die
//! NG_BACKDROP=white|black  flat backdrop window behind, for the material transmission test
//! NG_NOBELOW=1           do not adopt the host's Layer::Below container
//! NG_NOSIDEBAR=1         no sidebar at all (baseline geometry)
//! NG_COLLAPSED=1         start collapsed
//! NG_TOGGLE_EVERY=n      toggle every n frames, so a capture lands mid-animation
//! NG_NOPROBES=1          omit the two NativeHost layer probes
//! NG_PROBE=1             print the AppKit probe + geometry table every 30 frames
//! NG_RESIZE=1            scripted resize sweep, logging logical-vs-drawable per frame
//! NG_TOGGLE=1            scripted collapse/expand, logging every animation frame
//! NG_CHURN=1             force GPUI re-renders and re-push the model every frame
//! NG_STATE=1             after the row clicks: collapse a group, scroll, then churn
//! NG_ROWS=n              pad the last group with n extra rows so the list scrolls
//! NG_CLICK=x,y;x,y       post synthetic clicks at window-content points, restoring between
//! NG_ROWCLICK=n:id,n:id  click sidebar row n, expecting selection `id`
//! NG_EDGE=1              pane-edge click sweep, expanded / after a resize / collapsed / mid-
//!                        animation, across the pane's left edge, mid-pane and the right edge
//! NG_EDGE_HOLD=1         with NG_EDGE: paint the sweep's targets and post no input
//! NG_MUTATE=1            after 120 frames: badge change, insert, remove, reorder
//! NG_QUIT=<frames>       exit after N frames
//! ```

use std::env;
use std::ffi::c_void;
use std::time::Instant;

use gpui::{
    App, Application, Bounds, Context, MouseButton, Render, SharedString, Window, WindowBounds,
    WindowOptions, div, point, prelude::*, px, rgb, rgba, size,
};
use native_gpui::sidebar::{
    NativeSidebar, PaneAlign, SidebarDiff, SidebarItem, SidebarMetrics, SidebarOptions,
    sidebar_group, sidebar_item,
};
use native_gpui::{Layer, NativeHost, native_root, window_options};
use objc2::rc::Retained;
use objc2::{MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

const WIN_W: f32 = 1000.;
const WIN_H: f32 = 640.;
/// Where the two `NativeHost` probes go, in GPUI-logical points relative to the gutter.
const PROBE_ABOVE: (f32, f32, f32, f32) = (500., 24., 64., 64.);
const PROBE_BELOW: (f32, f32, f32, f32) = (500., 108., 168., 96.);
/// Hairline comb period, in GPUI-logical points.
const COMB: f32 = 47.;

#[repr(C)]
#[derive(Copy, Clone)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        position: CGPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    fn CFRelease(cf: *const c_void);
}

const TAP_HID: u32 = 0;
const EV_MOUSE_MOVED: u32 = 5;
const EV_LEFT_DOWN: u32 = 1;
const EV_LEFT_UP: u32 = 2;

fn pointer_now() -> CGPoint {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(event);
        CFRelease(event);
        at
    }
}

fn post(kind: u32, at: CGPoint) {
    unsafe {
        let event = CGEventCreateMouseEvent(std::ptr::null(), kind, at, 0);
        CGEventPost(TAP_HID, event);
        CFRelease(event);
    }
}

/// One click at a global top-left-origin point, pointer restored before returning.
///
/// `CGWarpMouseCursorPosition` moves the cursor but posts no event of its own, so the move has
/// to be posted separately or nothing hovers.
fn click_at(global: CGPoint) {
    let was = pointer_now();
    unsafe { CGWarpMouseCursorPosition(global) };
    post(EV_MOUSE_MOVED, global);
    std::thread::sleep(std::time::Duration::from_millis(40));
    post(EV_LEFT_DOWN, global);
    std::thread::sleep(std::time::Duration::from_millis(30));
    post(EV_LEFT_UP, global);
    std::thread::sleep(std::time::Duration::from_millis(30));
    unsafe { CGWarpMouseCursorPosition(was) };
    post(EV_MOUSE_MOVED, was);
    println!(
        "CLICK posted at global ({:.1},{:.1}); pointer restored to ({:.1},{:.1})",
        global.x, global.y, was.x, was.y
    );
}

/// Height of the screen whose bottom-left is Cocoa's global origin.
///
/// Not `CGDisplayPixelsHigh`: that reports the display mode's *pixel* height, which on a scaled
/// Retina display is not the point height, and the flip comes out wrong by hundreds of points.
fn screen_height() -> f64 {
    unsafe { NG_SCREEN_HEIGHT }
}
static mut NG_SCREEN_HEIGHT: f64 = 0.0;

/// First hit-target row's logical y, and the spacing between rows. A band nothing else paints in.
const EDGE_Y0: f64 = 386.;
const EDGE_DY: f64 = 16.;
/// Offsets inside the **window's** left edge for the left-strip probe. The last is the
/// known-positive: far enough in that nothing can be claiming it.
const EDGE_BORDER: [f64; 5] = [1., 2., 3., 5., 12.];
/// The mid-animation target's logical rect and aimed point.
///
/// Fixed in GPUI-logical space and deliberately not derived from the gutter: a target placed
/// relative to `metrics.left` would move while the pane animates, racing the thing being
/// measured. At a fixed logical x it is stationary in the aligned arm and moves only in the
/// broken one.
const EDGE_ANIM: (f32, f32, f32, f32) = (690., 292., 22., 22.);
const EDGE_ANIM_AIM: (f64, f64) = (700., 303.);

/// Where GPUI's logical (0,0) is actually drawn, in window-content top-left points: the pane's
/// origin plus GPUI's frame origin, flipped.
///
/// Aiming at `draw_origin + logical` is what makes the sweep falsifiable. Aiming at the logical
/// point directly — what `NG_CLICK` does — posts at the same global place in both arms, so both
/// report zero error and the control fabricates a pass.
fn draw_origin(sidebar: &NativeSidebar) -> (f64, f64) {
    let pane = sidebar.pane_rect();
    let gpui = sidebar.gpui_frame();
    let cv = sidebar.content_view_size();
    (
        pane.origin.x + gpui.origin.x,
        cv.height - (pane.origin.y + gpui.origin.y + gpui.size.height),
    )
}

/// A click posted from a worker thread, so the main thread keeps running AppKit's animation.
///
/// [`click_at`] sleeps on the calling thread, which on the main thread stalls the very collapse
/// the click is aimed into. `CGEventPost` is thread-safe.
fn click_async(global: CGPoint, delay_ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let was = pointer_now();
        unsafe { CGWarpMouseCursorPosition(global) };
        post(EV_MOUSE_MOVED, global);
        std::thread::sleep(std::time::Duration::from_millis(10));
        post(EV_LEFT_DOWN, global);
        std::thread::sleep(std::time::Duration::from_millis(10));
        post(EV_LEFT_UP, global);
        std::thread::sleep(std::time::Duration::from_millis(10));
        unsafe { CGWarpMouseCursorPosition(was) };
        post(EV_MOUSE_MOVED, was);
    });
}

/// One GPUI hit target: a painted rect and the logical point aimed at inside it.
#[derive(Clone)]
struct EdgeTarget {
    label: &'static str,
    rect: (f32, f32, f32, f32),
    aim: (f64, f64),
    /// Points inside the pane's left edge, for the left-edge profile rows only.
    dx: Option<f64>,
}

impl EdgeTarget {
    /// Row `k`'s 20x14 rect, holding `ax` one point in from the rect's own left edge so a point
    /// aimed flush with the pane's edge is still inside its target. Clamped clear of the strip.
    fn new(label: &'static str, ax: f64, k: usize, left: f64, w: f64, dx: Option<f64>) -> Self {
        let ay = EDGE_Y0 + k as f64 * EDGE_DY;
        let rx = (ax - 1.).clamp(left, (w - 20.).max(left));
        Self {
            label,
            rect: (rx as f32, (ay - 7.) as f32, 20., 14.),
            aim: (ax, ay),
            dx,
        }
    }
}

/// Targets across the pane, in GPUI-logical points, built from the gutter the sidebar reports.
///
/// Both edges get a *profile* rather than spot checks: a single null cannot say what swallowed
/// the click, but the offset at which clicks start arriving can, and every landing offset is a
/// known-positive proving the stimulus does reach the window at that x. Each offset gets its own
/// y row, since a divider band and a window resize border are both vertical strips.
fn edge_targets(sidebar: &NativeSidebar) -> Vec<EdgeTarget> {
    let left = f64::from(f32::from(sidebar.metrics().left));
    let w = sidebar.content_view_size().width;
    let mut out: Vec<EdgeTarget> = [
        ("edge+1", 1.),
        ("edge+2", 2.),
        ("edge+3", 3.),
        ("edge+4", 4.),
        ("edge+6", 6.),
        ("edge+8", 8.),
    ]
    .into_iter()
    .enumerate()
    .map(|(k, (label, dx))| EdgeTarget::new(label, left + dx, k, left, w, Some(dx)))
    .collect();
    out.push(EdgeTarget::new(
        "mid",
        left + (w - left) / 2.,
        6,
        left,
        w,
        None,
    ));
    out.extend(
        [
            ("right-1", 1.),
            ("right-2", 2.),
            ("right-3", 3.),
            ("right-4", 4.),
            ("right-6", 6.),
            ("right-8", 8.),
            ("right-16", 16.),
        ]
        .into_iter()
        .enumerate()
        .map(|(k, (label, dx))| EdgeTarget::new(label, w - dx, 7 + k, left, w, None)),
    );
    out
}

/// What GPUI reported for one received mouse-down, with the geometry read at the same instant.
#[derive(Copy, Clone)]
struct Recv {
    x: f32,
    y: f32,
    draw: (f64, f64),
    pane_x: f64,
    aligns: u64,
    corrections: u64,
}

struct EdgeRow {
    state: &'static str,
    label: &'static str,
    aim: (f64, f64),
    posted: (f64, f64),
    got: Option<(f32, f32)>,
    hit: bool,
    onscreen: bool,
    tries: u32,
    sel: Option<String>,
    /// Points inside the pane's left edge, for the left-edge profile rows only.
    dx: Option<f64>,
}

#[derive(Default)]
struct Edge {
    phase: usize,
    wait: u64,
    state: &'static str,
    targets: Vec<EdgeTarget>,
    ti: usize,
    tries: u32,
    /// Watermark on `recv_n`, so a late event from the previous target is never read as this
    /// target's answer.
    seen: usize,
    recv_n: usize,
    recv: Option<Recv>,
    hit: Option<usize>,
    anim_hit: bool,
    anim_frames: u64,
    rows: Vec<EdgeRow>,
    border_i: usize,
    border_tries: u32,
    /// Offset inside the window's left edge, and whether AppKit's own outline view answered.
    border_rows: Vec<(f64, bool, u32)>,
}

fn edge_readout(when: &str, sidebar: &NativeSidebar) {
    let m = sidebar.metrics();
    let g = sidebar.gpui_frame();
    let cv = sidebar.content_view_size();
    let pane = sidebar.pane_rect();
    let draw = draw_origin(sidebar);
    println!(
        "EDGE-GEOM {:<22} collapsed={:<5} left {:7.2} | pane x {:7.2} w {:7.2} | gpui origin ({:8.2},{:7.2}) size {:7.2}x{:7.2} | contentView {:7.2}x{:7.2} | logical==drawable {:<5} | drawOrigin ({:+.2},{:+.2}) | divider {:.2} | align_count {} frame_corrections {}",
        when,
        m.collapsed,
        f32::from(m.left),
        pane.origin.x,
        pane.size.width,
        g.origin.x,
        g.origin.y,
        g.size.width,
        g.size.height,
        cv.width,
        cv.height,
        (g.size.width - cv.width).abs() < 0.5 && (g.size.height - cv.height).abs() < 0.5,
        draw.0,
        draw.1,
        sidebar.probe().divider_thickness,
        sidebar.align_count(),
        sidebar.frame_corrections(),
    );
    // Where the rows start, in the space the left-strip probe aims in: without it, a null from
    // that probe cannot be told from a click that landed on the sidebar item's own background.
    if let (Some(p), Some(l)) = (sidebar.row_point(1), sidebar.row_layout(1)) {
        println!(
            "EDGE-ROW1  {:<22} row 1 centre in contentView ({:7.2},{:7.2}) | row rect in outline ({:.2},{:.2}) {:.2}x{:.2} | rows start at contentView x = {:.2}",
            when,
            p.x,
            p.y,
            l.row.origin.x,
            l.row.origin.y,
            l.row.size.width,
            l.row.size.height,
            p.x - l.row.size.width / 2.,
        );
    }
}

/// Drive one target per call. Returns true when the pass is finished.
fn edge_pass(edge: &mut Edge, sidebar: &NativeSidebar) -> bool {
    if edge.ti >= edge.targets.len() {
        edge.ti = 0;
        edge.tries = 0;
        return true;
    }
    let t = edge.targets[edge.ti].clone();
    let draw = draw_origin(sidebar);
    let cv = sidebar.content_view_size();
    let posted = (draw.0 + t.aim.0, draw.1 + t.aim.1);
    let onscreen = posted.0 >= 0. && posted.0 < cv.width && posted.1 >= 0. && posted.1 < cv.height;
    let sel = sidebar.selected().map(|s| s.to_string());

    if edge.recv_n > edge.seen {
        let got = edge.recv.expect("recv_n rose, so recv is set");
        let hit = edge.hit == Some(edge.ti);
        println!(
            "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) posted ({:7.2},{:7.2}) got ({:7.2},{:7.2}) err ({:+6.2},{:+6.2}) hit {:<5} drawOrigin-at-receipt ({:+.2},{:+.2}) paneX {:7.2} tries {} sel {:?}",
            edge.state,
            t.label,
            t.aim.0,
            t.aim.1,
            posted.0,
            posted.1,
            got.x,
            got.y,
            got.x as f64 - t.aim.0,
            got.y as f64 - t.aim.1,
            hit,
            got.draw.0,
            got.draw.1,
            got.pane_x,
            edge.tries,
            sel,
        );
        edge.rows.push(EdgeRow {
            state: edge.state,
            label: t.label,
            aim: t.aim,
            posted,
            got: Some((got.x, got.y)),
            hit,
            onscreen,
            tries: edge.tries,
            sel,
            dx: t.dx,
        });
        edge.seen = edge.recv_n;
        edge.hit = None;
        edge.ti += 1;
        edge.tries = 0;
        edge.wait = 8;
        return false;
    }

    if !onscreen {
        println!(
            "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) posted ({:7.2},{:7.2}) OFF-WINDOW (contentView {:.0}x{:.0}): the target is clipped away, so there is nothing to click",
            edge.state, t.label, t.aim.0, t.aim.1, posted.0, posted.1, cv.width, cv.height,
        );
        edge.rows.push(EdgeRow {
            state: edge.state,
            label: t.label,
            aim: t.aim,
            posted,
            got: None,
            hit: false,
            onscreen: false,
            tries: 0,
            sel,
            dx: t.dx,
        });
        edge.ti += 1;
        edge.tries = 0;
        edge.wait = 4;
        return false;
    }

    if edge.tries >= 6 {
        println!(
            "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) posted ({:7.2},{:7.2}) NOT RECEIVED after 6 attempts (window key {}, selected {:?}) — swallowed before GPUI, not mis-landed",
            edge.state,
            t.label,
            t.aim.0,
            t.aim.1,
            posted.0,
            posted.1,
            sidebar.window_is_key(),
            sel,
        );
        edge.rows.push(EdgeRow {
            state: edge.state,
            label: t.label,
            aim: t.aim,
            posted,
            got: None,
            hit: false,
            onscreen: true,
            tries: edge.tries,
            sel,
            dx: t.dx,
        });
        edge.ti += 1;
        edge.tries = 0;
        edge.wait = 8;
        return false;
    }

    edge.tries += 1;
    click_at(window_to_global(sidebar, posted.0, posted.1));
    edge.wait = 10;
    false
}

/// Log the alignment on every frame of a collapse/expand animation, and pick up the one click
/// posted asynchronously into it. Returns true when the animation window has elapsed.
fn edge_anim(edge: &mut Edge, sidebar: &NativeSidebar) -> bool {
    edge.anim_frames += 1;
    let draw = draw_origin(sidebar);
    let pane = sidebar.pane_rect();
    println!(
        "EDGE-ANIM {:<10} f{:<3} collapsed={:<5} paneX {:7.2} paneW {:7.2} left {:7.2} drawOrigin ({:+.2},{:+.2}) aligns {} corr {}",
        edge.state,
        edge.anim_frames,
        sidebar.is_collapsed(),
        pane.origin.x,
        pane.size.width,
        f32::from(sidebar.metrics().left),
        draw.0,
        draw.1,
        sidebar.align_count(),
        sidebar.frame_corrections(),
    );
    if edge.recv_n > edge.seen {
        let got = edge.recv.expect("recv_n rose, so recv is set");
        println!(
            "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) got ({:7.2},{:7.2}) err ({:+6.2},{:+6.2}) hit {:<5} drawOrigin-at-receipt ({:+.2},{:+.2}) paneX-at-receipt {:7.2} aligns {} corr {}",
            edge.state,
            "anim@700",
            EDGE_ANIM_AIM.0,
            EDGE_ANIM_AIM.1,
            got.x,
            got.y,
            got.x as f64 - EDGE_ANIM_AIM.0,
            got.y as f64 - EDGE_ANIM_AIM.1,
            edge.anim_hit,
            got.draw.0,
            got.draw.1,
            got.pane_x,
            got.aligns,
            got.corrections,
        );
        edge.rows.push(EdgeRow {
            state: edge.state,
            label: "anim@700",
            aim: EDGE_ANIM_AIM,
            posted: EDGE_ANIM_AIM,
            got: Some((got.x, got.y)),
            hit: edge.anim_hit,
            onscreen: true,
            tries: 1,
            sel: sidebar.selected().map(|s| s.to_string()),
            dx: None,
        });
        edge.seen = edge.recv_n;
        edge.anim_hit = false;
    }
    edge.anim_frames >= 42
}

fn edge_table(edge: &Edge, sidebar: &NativeSidebar) {
    println!("\n=== EDGE SWEEP TABLE (arm {}) ===", edge_arm());
    println!(
        "{:<12} {:<9} {:>17} {:>17} {:>17} {:>16} {:<5} {:>4}  {}",
        "state",
        "target",
        "aimed logical",
        "posted window",
        "received logical",
        "error (pt)",
        "hit",
        "att",
        "note"
    );
    let mut worst = 0f64;
    let mut received = 0usize;
    let mut hits = 0usize;
    for r in &edge.rows {
        let (got, err, note) = match r.got {
            Some((gx, gy)) => {
                received += 1;
                if r.hit {
                    hits += 1;
                }
                let ex = gx as f64 - r.aim.0;
                let ey = gy as f64 - r.aim.1;
                worst = worst.max(ex.abs()).max(ey.abs());
                (
                    format!("({:7.2},{:7.2})", gx, gy),
                    format!("({:+6.2},{:+6.2})", ex, ey),
                    if r.hit { "" } else { "wrong element" },
                )
            }
            None => (
                "not received".to_string(),
                "—".to_string(),
                if r.onscreen {
                    "swallowed"
                } else {
                    "off-window"
                },
            ),
        };
        println!(
            "{:<12} {:<9} {:>17} {:>17} {:>17} {:>16} {:<5} {:>4}  {}",
            r.state,
            r.label,
            format!("({:7.2},{:7.2})", r.aim.0, r.aim.1),
            format!("({:7.2},{:7.2})", r.posted.0, r.posted.1),
            got,
            err,
            r.hit,
            r.tries,
            note,
        );
    }
    println!(
        "SUMMARY arm={} rows={} received={} landed-on-target={} worst |error| among received = {:.2} pt",
        edge_arm(),
        edge.rows.len(),
        received,
        hits,
        worst,
    );

    // The offset at which the left edge starts answering, per state — a band a couple of points
    // wide is a tracking region, a null all the way across would be a broken hit path.
    let mut states: Vec<&str> = edge.rows.iter().map(|r| r.state).collect();
    states.dedup();
    for state in states {
        let rows: Vec<&EdgeRow> = edge
            .rows
            .iter()
            .filter(|r| r.state == state && r.dx.is_some())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let first = rows
            .iter()
            .filter(|r| r.got.is_some())
            .filter_map(|r| r.dx)
            .fold(f64::INFINITY, f64::min);
        let swallowed: Vec<f64> = rows
            .iter()
            .filter(|r| r.got.is_none())
            .filter_map(|r| r.dx)
            .collect();
        println!(
            "LEFT-EDGE PROFILE {state:<12} first offset that reached GPUI = {} pt inside the pane edge; swallowed at {swallowed:?} pt",
            if first.is_finite() {
                format!("{first:.0}")
            } else {
                "none of them".to_string()
            },
        );
    }
    if !edge.border_rows.is_empty() {
        let answered: Vec<f64> = edge
            .border_rows
            .iter()
            .filter(|(_, ok, _)| *ok)
            .map(|(dx, _, _)| *dx)
            .collect();
        let eaten: Vec<f64> = edge
            .border_rows
            .iter()
            .filter(|(_, ok, _)| !*ok)
            .map(|(dx, _, _)| *dx)
            .collect();
        println!(
            "LEFT-STRIP PROBE (sidebar expanded, aimed at row 1 inside the sidebar strip, detector = AppKit selection): the outline view answered at {answered:?} pt inside the window's left edge and nothing arrived at {eaten:?} pt. Read with EDGE-ROW1: rows start 8pt in, so this bounds a band dead to BOTH hit paths and does not isolate a window frame border"
        );
    }
    let mut sels: Vec<String> = edge
        .rows
        .iter()
        .map(|r| match &r.sel {
            Some(s) => s.clone(),
            None => "none".to_string(),
        })
        .collect();
    sels.sort();
    sels.dedup();
    println!(
        "SIDEBAR SELECTION observed across the sweep: {sels:?} — a click that reached the outline view instead of GPUI would have changed it"
    );
    edge_readout("final", sidebar);
}

fn edge_arm() -> &'static str {
    if env::var("NG_ARM").as_deref() == Ok("fill") {
        "fill (broken control)"
    } else {
        "aligned (shipped)"
    }
}

/// The sweep's state machine. One step per GPUI frame.
fn edge_step(edge: &mut Edge, sidebar: &NativeSidebar, window: &mut Window, cx: &mut App) {
    if edge.wait > 0 {
        edge.wait -= 1;
        return;
    }
    match edge.phase {
        0 => {
            println!("=== EDGE SWEEP begins, arm {} ===", edge_arm());
            edge_readout("at rest", sidebar);
            cx.activate(true);
            edge.state = "expanded";
            edge.targets = edge_targets(sidebar);
            for t in &edge.targets {
                println!(
                    "  target {:<8} logical rect ({:.1},{:.1},{:.1}x{:.1}) aim ({:.2},{:.2})",
                    t.label, t.rect.0, t.rect.1, t.rect.2, t.rect.3, t.aim.0, t.aim.1
                );
            }
            // Captures and synthetic input cannot share a run: a `screencapture` loop silently
            // stops synthetic clicks being delivered, so a capture pass posts nothing at all.
            if env_flag("NG_EDGE_HOLD") {
                println!("EDGE HOLD: targets painted, no input will be posted");
                edge.phase = 100;
            } else {
                edge.phase = 1;
                edge.wait = 30;
            }
        }
        1 => {
            if edge_pass(edge, sidebar) {
                edge.phase = 2;
            }
        }
        2 => {
            println!("--- resize to 1240x760 ---");
            window.resize(gpui::size(px(1240.), px(760.)));
            edge.phase = 3;
            edge.wait = 25;
        }
        3 => {
            edge_readout("resized 1240x760", sidebar);
            println!("--- resize back to {WIN_W:.0}x{WIN_H:.0} ---");
            window.resize(gpui::size(px(WIN_W), px(WIN_H)));
            edge.phase = 4;
            edge.wait = 25;
        }
        4 => {
            edge_readout("after resize", sidebar);
            edge.state = "after-resize";
            edge.targets = edge_targets(sidebar);
            edge.phase = 5;
            edge.wait = 10;
        }
        5 => {
            if edge_pass(edge, sidebar) {
                edge.phase = 6;
            }
        }
        6 => {
            // Queue the click first, then start the animation, so it lands mid-collapse.
            let draw = draw_origin(sidebar);
            let at = window_to_global(sidebar, draw.0 + EDGE_ANIM_AIM.0, draw.1 + EDGE_ANIM_AIM.1);
            println!(
                "--- collapsing; async click queued at global ({:.1},{:.1}) for logical ({:.1},{:.1}) ---",
                at.x, at.y, EDGE_ANIM_AIM.0, EDGE_ANIM_AIM.1
            );
            edge.state = "collapsing";
            edge.anim_frames = 0;
            click_async(at, 60);
            sidebar.toggle();
            edge.phase = 7;
        }
        7 => {
            if edge_anim(edge, sidebar) {
                edge_readout("after collapse", sidebar);
                edge.state = "collapsed";
                edge.targets = edge_targets(sidebar);
                for t in &edge.targets {
                    println!(
                        "  target {:<8} logical rect ({:.1},{:.1},{:.1}x{:.1}) aim ({:.2},{:.2})",
                        t.label, t.rect.0, t.rect.1, t.rect.2, t.rect.3, t.aim.0, t.aim.1
                    );
                }
                edge.phase = 8;
                edge.wait = 10;
            }
        }
        8 => {
            if edge_pass(edge, sidebar) {
                edge.phase = 9;
            }
        }
        9 => {
            let draw = draw_origin(sidebar);
            let at = window_to_global(sidebar, draw.0 + EDGE_ANIM_AIM.0, draw.1 + EDGE_ANIM_AIM.1);
            println!(
                "--- expanding; async click queued at global ({:.1},{:.1}) for logical ({:.1},{:.1}) ---",
                at.x, at.y, EDGE_ANIM_AIM.0, EDGE_ANIM_AIM.1
            );
            edge.state = "expanding";
            edge.anim_frames = 0;
            click_async(at, 60);
            sidebar.toggle();
            edge.phase = 10;
        }
        10 => {
            if edge_anim(edge, sidebar) {
                edge_readout("after expand", sidebar);
                edge.state = "re-expanded";
                edge.targets = edge_targets(sidebar);
                edge.phase = 11;
                edge.wait = 10;
            }
        }
        11 => {
            if edge_pass(edge, sidebar) {
                edge.phase = 12;
            }
        }
        // Left-strip probe: is the window's far-left band clickable by anything at all? Aimed
        // into the sidebar strip at row 1's centre with the sidebar expanded, so the divider is
        // 248pt away; the detector is AppKit's own selection.
        //
        // It cannot isolate a window frame border, and `EDGE-ROW1` is printed so nobody reads it
        // that way: rows start 8pt inside the window, so every smaller offset lands on the
        // sidebar item's own background whether or not a frame border also claims it. All this
        // establishes is that the band is dead to both hit paths.
        12 => {
            if edge.border_i >= EDGE_BORDER.len() {
                edge.phase = 13;
                return;
            }
            let dx = EDGE_BORDER[edge.border_i];
            if sidebar.selected().is_some_and(|s| s == "inbox") {
                println!(
                    "EDGE-STRIP {dx:>4.0} pt inside the window's left edge -> outline view selected `inbox` after {} attempt(s)",
                    edge.border_tries
                );
                edge.border_rows.push((dx, true, edge.border_tries));
                sidebar.set_selected(None);
                edge.border_i += 1;
                edge.border_tries = 0;
                edge.wait = 12;
                return;
            }
            if edge.border_tries >= 3 {
                println!(
                    "EDGE-STRIP {dx:>4.0} pt inside the window's left edge -> nothing reached the outline view in 3 attempts (window key {})",
                    sidebar.window_is_key()
                );
                edge.border_rows.push((dx, false, edge.border_tries));
                edge.border_i += 1;
                edge.border_tries = 0;
                edge.wait = 12;
                return;
            }
            let Some(at) = sidebar.row_point(1) else {
                println!("EDGE-STRIP row 1 is not laid out; probe skipped");
                edge.phase = 13;
                return;
            };
            edge.border_tries += 1;
            click_at(window_to_global(sidebar, dx, at.y));
            edge.wait = 14;
        }
        // Capture pose: paint the targets and wait, posting nothing.
        100 => {}
        _ => {
            edge_table(edge, sidebar);
            println!("EDGE SWEEP done");
            std::process::exit(0);
        }
    }
}

fn env_flag(key: &str) -> bool {
    env::var(key).map(|v| v != "0").unwrap_or(false)
}

/// ~12 rows over 3 groups, each leaf with a real SF Symbol name.
fn items(extra: bool, renamed: bool, reordered: bool) -> Vec<SidebarItem> {
    let inbox = sidebar_item("inbox", "Inbox").symbol("tray.full").badge(12);
    let sent = sidebar_item("sent", "Sent").symbol("paperplane");
    let drafts = if renamed {
        sidebar_item("drafts", "Drafts").symbol("doc.text").badge(3)
    } else {
        sidebar_item("drafts", "Drafts").symbol("doc.text")
    };
    let mut mailboxes = vec![inbox, sent, drafts];
    if reordered {
        mailboxes.swap(0, 2);
    }
    if extra {
        mailboxes.push(sidebar_item("archive", "Archive").symbol("archivebox"));
    }

    let mut smart = vec![
        sidebar_item("flagged", "Flagged").symbol("flag").badge(4),
        sidebar_item("unread", "Unread")
            .symbol("envelope.badge")
            .badge(37),
        sidebar_item("attachments", "Attachments").symbol("paperclip"),
        sidebar_item("today", "Today").symbol("calendar"),
    ];
    if !extra {
        smart.pop();
    }

    vec![
        sidebar_group("g-mailboxes", "Mailboxes").children(mailboxes),
        sidebar_group("g-smart", "Smart Mailboxes").children(smart),
        sidebar_group("g-tags", "Tags").children(
            [
                sidebar_item("work", "Work").symbol("circle.fill"),
                sidebar_item("home", "Home").symbol("house"),
                sidebar_item("travel", "Travel").symbol("airplane"),
                sidebar_item("later", "Read Later").symbol("bookmark"),
                sidebar_item("plain", "No Icon Row"),
            ]
            .into_iter()
            .chain(
                (0..pad()).map(|i| {
                    sidebar_item(format!("pad-{i}"), format!("Tag {}", i + 1)).symbol("tag")
                }),
            ),
        ),
    ]
}

fn pad() -> usize {
    env::var("NG_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

struct Demo {
    installed: bool,
    sidebar: Option<NativeSidebar>,
    selected: Option<SharedString>,
    selections: Vec<SharedString>,
    frames: u64,
    last_click: Option<(f32, f32)>,
    clicks: Vec<(f32, f32)>,
    last_diff: SidebarDiff,
    extra: bool,
    renamed: bool,
    reordered: bool,
    quit_at: Option<u64>,
    t0: Instant,
    click_points: Vec<(f64, f64)>,
    click_index: usize,
    click_tries: u32,
    click_seen: usize,
    click_wait: u64,
    row_click_rows: Vec<isize>,
    row_click_expect: Vec<String>,
    row_click_index: usize,
    row_click_tries: u32,
    state_frames: u64,
    writes_at_mark: u64,
    renders_at_mark: u64,
    backdrop: Option<Retained<NSWindow>>,
    /// `NG_EDGE` pane-edge click sweep. `None` when the arm is off.
    edge: Option<Edge>,
}

impl Demo {
    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if env_flag("NG_NOSIDEBAR") {
            println!("NO SIDEBAR arm: gpui alone");
            return;
        }
        let align = if env::var("NG_ARM").as_deref() == Ok("fill") {
            PaneAlign::FillPane
        } else {
            PaneAlign::WindowAligned
        };
        let options = SidebarOptions {
            align,
            adopt_below_layer: !env_flag("NG_NOBELOW"),
            collapsed: env_flag("NG_COLLAPSED"),
            ..Default::default()
        };
        println!(
            "INSTALL align={align:?} adopt_below={}",
            options.adopt_below_layer
        );

        let sidebar = NativeSidebar::install(window, cx, options);
        // Idempotence: a second call must not stack a second split controller.
        let again = NativeSidebar::install(window, cx, SidebarOptions::default());
        println!(
            "idempotent: subviews of window contentView = {}, second install same handle = {}",
            sidebar.split_subview_count(),
            sidebar.window_number() == again.window_number()
        );

        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, _, cx| {
            println!("SELECT -> {id:?}");
            if let Some(id) = id {
                this.selections.push(id.clone());
            }
            this.selected = id.clone();
            cx.notify();
        }));
        let diff = sidebar.update(&items(self.extra, self.renamed, self.reordered));
        let frame = sidebar.window_frame();
        unsafe { NG_SCREEN_HEIGHT = sidebar.screen_height() };
        // For the capture script: `screencapture -l` needs the CGWindowID, and both numbers come
        // from inside the process, so they cannot describe some other window.
        println!(
            "WINDOW id={} frame=({:.0},{:.0},{:.0},{:.0}) scale={:.1}",
            sidebar.window_number(),
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
            2.0,
        );
        println!("SCREEN height={:.0}", sidebar.screen_height());
        println!("first update: {diff:?}");
        self.sidebar = Some(sidebar);
    }

    fn probe(&self) {
        let Some(sidebar) = &self.sidebar else { return };
        let probe = sidebar.probe();
        let metrics = sidebar.metrics();
        let gpui = sidebar.gpui_frame();
        let content = sidebar.content_view_size();
        let pane = sidebar.pane_rect();
        println!(
            "f{:<5} behavior={} fullHeight={} style={}/{} rowH={:.1} indent={:.1} divider={:.2} rows={} toolbarItems={} scrollDrawsBg={} outlineFirstResponder={}",
            self.frames,
            probe.behavior,
            probe.allows_full_height_layout,
            probe.style,
            probe.effective_style,
            probe.row_height,
            probe.indentation_per_level,
            probe.divider_thickness,
            probe.rows,
            probe.toolbar_items,
            probe.scroll_draws_background,
            probe.outline_is_first_responder,
        );
        println!(
            "      metrics left={:.2} top={:.2} thickness={:.2} collapsed={} | pane ({:.2},{:.2}) {:.2}x{:.2} | gpui origin ({:.2},{:.2}) size {:.2}x{:.2} | contentView {:.2}x{:.2} | logical==drawable {} | aligns={}",
            f32::from(metrics.left),
            f32::from(metrics.top),
            f32::from(metrics.thickness),
            metrics.collapsed,
            pane.origin.x,
            pane.origin.y,
            pane.size.width,
            pane.size.height,
            gpui.origin.x,
            gpui.origin.y,
            gpui.size.width,
            gpui.size.height,
            content.width,
            content.height,
            (gpui.size.width - content.width).abs() < 0.5
                && (gpui.size.height - content.height).abs() < 0.5,
            sidebar.align_count(),
        );
        println!("      frame_corrections={}", sidebar.frame_corrections());
        for row in 0..probe.rows.min(6) {
            if let Some(l) = sidebar.row_layout(row) {
                println!(
                    "      row{} level={} expanded={} row=({:.1},{:.1},{:.1}x{:.1}) cell=({:.1},{:.1},{:.1}x{:.1}) icon=({:.1},{:.1},{:.1}x{:.1}) title=({:.1},{:.1},{:.1}x{:.1}) badge=({:.1},{:.1},{:.1}x{:.1})",
                    row,
                    l.level,
                    l.expanded,
                    l.row.origin.x,
                    l.row.origin.y,
                    l.row.size.width,
                    l.row.size.height,
                    l.cell.origin.x,
                    l.cell.origin.y,
                    l.cell.size.width,
                    l.cell.size.height,
                    l.icon.origin.x,
                    l.icon.origin.y,
                    l.icon.size.width,
                    l.icon.size.height,
                    l.title.origin.x,
                    l.title.origin.y,
                    l.title.size.width,
                    l.title.size.height,
                    l.badge.origin.x,
                    l.badge.origin.y,
                    l.badge.size.width,
                    l.badge.size.height,
                );
            }
        }
    }
}

/// A borderless flat-colour window glued behind the main window, 80pt larger on every side.
///
/// The sidebar's material is `behindWindow`, so it blurs what the WindowServer has behind the
/// window — which a window-scoped capture does not contain. Putting something known back there
/// and swinging it is the only way to measure that the blur is live. The swing has to be
/// full-field: AppKit samples the backdrop at one-eighth scale, far wider than any one band.
fn install_backdrop(sidebar: &NativeSidebar, white: bool) -> Retained<NSWindow> {
    let mtm = MainThreadMarker::new().expect("main thread");
    let f = sidebar.window_frame();
    let rect = NSRect::new(
        NSPoint::new(f.origin.x - 80., f.origin.y - 80.),
        NSSize::new(f.size.width + 160., f.size.height + 160.),
    );
    let win: Retained<NSWindow> = unsafe {
        msg_send![
            NSWindow::alloc(mtm),
            initWithContentRect: rect,
            styleMask: NSWindowStyleMask::Borderless,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    let level = if white { 1.0 } else { 0.0 };
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(level, level, level, 1.);
    win.setBackgroundColor(Some(&color));
    win.setOpaque(true);
    win.setHasShadow(false);
    win.orderFront(None);
    if let Some(main) = sidebar.window_frame_window() {
        unsafe { main.addChildWindow_ordered(&win, NSWindowOrderingMode::Below) };
    }
    win
}

/// A plain AppKit view with a flat layer colour, hosted by [`NativeHost`] at `layer`.
///
/// Three outcomes, not two: the probe's colour means present and correctly placed, the GPUI
/// panel colour means absent, and the colour anywhere else means present but misplaced.
fn host_probe(
    window: &mut Window,
    cx: &mut App,
    key: u64,
    layer: Layer,
    rect: (f32, f32, f32, f32),
    rgb: (f64, f64, f64),
) {
    let (x, y, w, h) = rect;
    let bounds = Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    };
    let mask = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(4000.), px(4000.)),
    };
    let host = NativeHost::get(window, cx);
    let _: Retained<NSView> = host.place(key, layer, bounds, mask, move |mtm, _| {
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(1., 1.)),
        );
        view.setWantsLayer(true);
        if let Some(l) = view.layer() {
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(rgb.0, rgb.1, rgb.2, 1.);
            l.setBackgroundColor(Some(&color.CGColor()));
        }
        view
    });
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.installed {
            self.installed = true;
            self.install(window, cx);
        }
        self.frames += 1;

        if let Some(sidebar) = self.sidebar.clone() {
            let diff = sidebar.update(&items(self.extra, self.renamed, self.reordered));
            if diff != self.last_diff {
                println!(
                    "f{} diff {diff:?} writes={}",
                    self.frames,
                    sidebar.appkit_writes()
                );
                self.last_diff = diff;
            }
            script(self, &sidebar, window, cx);
        }

        if env_flag("NG_CHURN")
            || env_flag("NG_PROBE")
            || env_flag("NG_RESIZE")
            || env_flag("NG_STATE")
            || env_flag("NG_TOGGLE")
            || env_flag("NG_MUTATE")
            || env::var("NG_CLICK").is_ok()
            || env::var("NG_ROWCLICK").is_ok()
            || env::var("NG_QUIT").is_ok()
            || env_flag("NG_EDGE")
        {
            window.request_animation_frame();
        }
        if env_flag("NG_PROBE") && self.frames % 30 == 1 {
            self.probe();
        }
        if let Some(limit) = self.quit_at {
            if self.frames >= limit {
                println!(
                    "EXIT after {} frames, {:.2}s",
                    self.frames,
                    self.t0.elapsed().as_secs_f64()
                );
                std::process::exit(0);
            }
        }

        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
        let left = f32::from(metrics.left);
        let top = f32::from(metrics.top);
        if let Ok(kind) = env::var("NG_BACKDROP") {
            if self.backdrop.is_none() {
                if let Some(sidebar) = &self.sidebar {
                    let win = install_backdrop(sidebar, kind == "white");
                    println!("BACKDROP {kind} installed");
                    self.backdrop = Some(win);
                }
            }
        }
        if !env_flag("NG_NOPROBES") {
            host_probe(
                window,
                cx,
                1,
                Layer::Above,
                (
                    left + PROBE_ABOVE.0,
                    top + PROBE_ABOVE.1,
                    PROBE_ABOVE.2,
                    PROBE_ABOVE.3,
                ),
                (1.0, 0.42, 0.0),
            );
            host_probe(
                window,
                cx,
                2,
                Layer::Below,
                (
                    left + PROBE_BELOW.0,
                    top + PROBE_BELOW.1,
                    PROBE_BELOW.2,
                    PROBE_BELOW.3,
                ),
                (0.0, 0.85, 0.35),
            );
        }
        let selected = self.selected.clone().unwrap_or_else(|| "—".into());
        let clicks = self.clicks.clone();
        let last_click = self.last_click;
        let frames = self.frames;

        // Everything below is placed absolutely from the reported gutter; nothing paints over the
        // sidebar strip, where a GPUI background would hide the system material.
        let mut root = div().relative().size_full();

        // Four pieces, leaving an unpainted hole exactly over the Layer::Below probe: a below
        // view is composited *under* GPUI's Metal content, so a background over it hides it.
        let (hx, hy, hw, hh) = (
            left + PROBE_BELOW.0,
            top + PROBE_BELOW.1,
            PROBE_BELOW.2,
            PROBE_BELOW.3,
        );
        let panel = rgb(0x16191d);
        for (x, y, w, h) in [
            (left, 0., WIN_W - left, hy),
            (left, hy + hh, WIN_W - left, WIN_H - hy - hh),
            (left, hy, hx - left, hh),
            (hx + hw, hy, WIN_W - hx - hw, hh),
        ] {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w.max(0.)))
                    .h(px(h.max(0.)))
                    .bg(panel),
            );
        }

        // 1pt hairlines every COMB pt of logical x, measuring offset AND stretch: an offset view
        // shifts them all equally, a drawable disagreeing with the logical size changes spacing.
        //
        // 47, not 50: the sidebar is 248pt, so a 50pt comb puts the correct prediction and the
        // broken one 2pt apart and both "fit". A period that does not divide the inset separates
        // them by 13pt or more.
        let mut comb = div().absolute().left(px(0.)).top(px(0.)).size_full();
        for k in 1..22 {
            comb = comb.child(
                div()
                    .absolute()
                    .left(px(k as f32 * COMB))
                    .top(px(0.))
                    .w(px(1.))
                    .h(px(WIN_H))
                    .bg(rgba(0xffffff99)),
            );
        }
        root = root.child(comb);

        // A click target flush to the pane's left edge, 4pt clear of the divider's tracking band.
        root = root.child(
            div()
                .absolute()
                .left(px(left + 4.))
                .top(px(top + 240.))
                .w(px(90.))
                .h(px(44.))
                .bg(rgb(0xffcc00)),
        );
        // Paired white-over-black occlusion gate, self-locating so a display capture can be
        // searched for it. A single-colour foreign window can pass at most one half.
        root = root.child(
            div()
                .absolute()
                .left(px(left + 320.))
                .top(px(top + 330.))
                .w(px(160.))
                .h(px(40.))
                .bg(rgb(0xffffff)),
        );
        root = root.child(
            div()
                .absolute()
                .left(px(left + 320.))
                .top(px(top + 370.))
                .w(px(160.))
                .h(px(40.))
                .bg(rgb(0x000000)),
        );

        // Selection round-trip: painted by GPUI from `self.selected`, so tracking the clicked row
        // proves the AppKit -> deferred dispatch -> GPUI state -> re-render path.
        root = root.child(
            div()
                .absolute()
                .left(px(left + 24.))
                .top(px(top + 24.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(28.))
                        .text_color(rgb(0xf2f2f7))
                        .child(format!("selected: {selected}")),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(0x9aa0a6))
                        .child(format!(
                        "gutter left {left:.1} top {top:.1} · frame {frames} · clicks received {}",
                        clicks.len()
                    )),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(0x9aa0a6))
                        .child(format!("history: {}", self.selections.join(" \u{2192} "))),
                ),
        );

        // A per-selection swatch, so a screenshot alone shows the selection changed.
        let swatch = match selected.as_ref() {
            "inbox" => 0x0a84ff,
            "sent" => 0x30d158,
            "drafts" => 0xff9f0a,
            "flagged" => 0xff453a,
            "unread" => 0xbf5af2,
            "attachments" => 0x64d2ff,
            "today" => 0xffd60a,
            "work" => 0xff6482,
            "home" => 0x66d4cf,
            "travel" => 0x5e5ce6,
            "later" => 0xac8e68,
            "plain" => 0x98989d,
            _ => 0x3a3a3c,
        };
        root = root.child(
            div()
                .absolute()
                .left(px(left + 24.))
                .top(px(top + 140.))
                .w(px(220.))
                .h(px(72.))
                .rounded(px(10.))
                .bg(rgb(swatch)),
        );

        if let Some((x, y)) = last_click {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x - 7.))
                    .top(px(y - 7.))
                    .size(px(14.))
                    .bg(rgb(0x00ff44)),
            );
        }
        // The sweep's hit targets, painted last so they sit on top. Each carries its own
        // `on_mouse_down`, which gpui fires only for an event inside that element's painted
        // bounds — so "landed on target" is gpui's hit test answering, not arithmetic here.
        if let Some(targets) = self.edge.as_ref().map(|e| e.targets.clone()) {
            for (k, t) in targets.iter().enumerate() {
                let (tx, ty, tw, th) = t.rect;
                root = root.child(
                    div()
                        .absolute()
                        .left(px(tx))
                        .top(px(ty))
                        .w(px(tw))
                        .h(px(th))
                        .bg(rgb(0x00e5ff))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &gpui::MouseDownEvent, _, _| {
                                if let Some(edge) = this.edge.as_mut() {
                                    edge.hit = Some(k);
                                }
                            }),
                        ),
                );
            }
            root = root.child(
                div()
                    .absolute()
                    .left(px(EDGE_ANIM.0))
                    .top(px(EDGE_ANIM.1))
                    .w(px(EDGE_ANIM.2))
                    .h(px(EDGE_ANIM.3))
                    .bg(rgb(0xff2d95))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &gpui::MouseDownEvent, _, _| {
                            if let Some(edge) = this.edge.as_mut() {
                                edge.anim_hit = true;
                            }
                        }),
                    ),
            );
        }

        native_root(root.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &gpui::MouseDownEvent, _, cx| {
                let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                println!("GPUI RECEIVED mouse down at ({x:.2}, {y:.2})");
                this.last_click = Some((x, y));
                this.clicks.push((x, y));
                // Snapshot the geometry in the same dispatch that delivered the event: read
                // afterwards it would describe a pane that has since moved.
                let sidebar = this.sidebar.clone();
                if let (Some(edge), Some(sidebar)) = (this.edge.as_mut(), sidebar) {
                    edge.recv_n += 1;
                    edge.recv = Some(Recv {
                        x,
                        y,
                        draw: draw_origin(&sidebar),
                        pane_x: sidebar.pane_rect().origin.x,
                        aligns: sidebar.align_count(),
                        corrections: sidebar.frame_corrections(),
                    });
                }
                cx.notify();
            }),
        ))
    }
}

/// Scripted arms, driven off the frame counter so every log line is attributable.
fn script(demo: &mut Demo, sidebar: &NativeSidebar, window: &mut Window, cx: &mut Context<Demo>) {
    let f = demo.frames;

    // The sweep owns the frame while it runs: it drives its own resize and collapse, and any
    // other arm moving the window underneath it would invalidate every aimed point.
    if let Some(mut edge) = demo.edge.take() {
        edge_step(&mut edge, sidebar, window, cx);
        demo.edge = Some(edge);
        return;
    }

    if env_flag("NG_RESIZE") {
        let size = match f {
            60 => Some(gpui::size(px(1240.), px(760.))),
            160 => Some(gpui::size(px(760.), px(520.))),
            260 => Some(gpui::size(px(WIN_W), px(WIN_H))),
            _ => None,
        };
        if let Some(size) = size {
            println!(
                "--- resize to {:.0}x{:.0} ---",
                f32::from(size.width),
                f32::from(size.height)
            );
            window.resize(size);
        }
        if (58..=78).contains(&f) || (158..=178).contains(&f) || (258..=278).contains(&f) {
            let gpui = sidebar.gpui_frame();
            let content = sidebar.content_view_size();
            println!(
                "f{:<4} contentView {:7.2}x{:7.2} | gpui origin ({:8.2},{:7.2}) size {:7.2}x{:7.2} | logical==drawable {} | left {:.2}",
                f,
                content.width,
                content.height,
                gpui.origin.x,
                gpui.origin.y,
                gpui.size.width,
                gpui.size.height,
                (gpui.size.width - content.width).abs() < 0.5
                    && (gpui.size.height - content.height).abs() < 0.5,
                f32::from(sidebar.metrics().left),
            );
        }
    }

    if let Ok(every) = env::var("NG_TOGGLE_EVERY") {
        if let Ok(every) = every.parse::<u64>() {
            if f > 40 && f % every == 0 {
                sidebar.toggle();
            }
        }
    }

    if env_flag("NG_TOGGLE") {
        if f == 60 || f == 200 {
            println!(
                "--- toggleSidebar (collapsed was {}) ---",
                sidebar.is_collapsed()
            );
            sidebar.toggle();
        }
        if (58..=140).contains(&f) || (198..=280).contains(&f) {
            let gpui = sidebar.gpui_frame();
            let content = sidebar.content_view_size();
            let pane = sidebar.pane_rect();
            // What THIS render pass lays out against. Tracking the pane means the deferred
            // publish really drives GPUI's layout every animation frame, not just at the ends.
            let global_left = cx
                .try_global::<SidebarMetrics>()
                .map(|m| f32::from(m.left))
                .unwrap_or(-1.);
            println!(
                "f{:<4} collapsed={} pane x {:7.2} w {:7.2} | left {:7.2} global {:7.2} | gpui origin ({:8.2},{:7.2}) size {:7.2}x{:7.2} | logical==drawable {}",
                f,
                sidebar.is_collapsed(),
                pane.origin.x,
                pane.size.width,
                f32::from(sidebar.metrics().left),
                global_left,
                gpui.origin.x,
                gpui.origin.y,
                gpui.size.width,
                gpui.size.height,
                (gpui.size.width - content.width).abs() < 0.5
                    && (gpui.size.height - content.height).abs() < 0.5,
            );
        }
    }

    if env_flag("NG_MUTATE") {
        match f {
            120 => {
                println!("--- badge change on `drafts` (content-only) ---");
                demo.renamed = true;
            }
            160 => {
                println!("--- insert `archive` + `today` (structural) ---");
                demo.extra = true;
            }
            200 => {
                println!("--- reorder mailboxes (structural, move) ---");
                demo.reordered = true;
            }
            240 => {
                println!("--- remove `archive` + `today` (structural) ---");
                demo.extra = false;
            }
            _ => {}
        }
        if [119, 121, 159, 161, 199, 201, 239, 241].contains(&f) {
            println!(
                "f{:<4} STATE selected={:?} g-mailboxes expanded={} g-tags expanded={} scroll={:.2} rows={}",
                f,
                sidebar.selected(),
                sidebar.is_expanded("g-mailboxes"),
                sidebar.is_expanded("g-tags"),
                sidebar.scroll_offset(),
                sidebar.probe().rows,
            );
        }
    }

    // Three separate things silently eat a synthetic click: `acceptsFirstMouse:` is NO by
    // default, a non-key window gets nothing, and a warp posts no event of its own. So each
    // click re-activates first and then waits for the app's own mouse-down count to rise. One
    // still unreceived while the window is verifiably key is a real swallow, reported as such.
    if demo.click_index < demo.click_points.len() {
        if demo.click_wait > 0 {
            demo.click_wait -= 1;
            return;
        }
        let landed = demo.clicks.len() > demo.click_seen;
        let (x, y) = demo.click_points[demo.click_index];
        if landed {
            let got = demo.clicks[demo.clicks.len() - 1];
            println!(
                "CLICK {} intent ({x:.2},{y:.2}) RECEIVED at ({:.2},{:.2}) err ({:+.2},{:+.2}) after {} attempt(s)",
                demo.click_index,
                got.0,
                got.1,
                got.0 as f64 - x,
                got.1 as f64 - y,
                demo.click_tries
            );
            demo.click_seen = demo.clicks.len();
            demo.click_index += 1;
            demo.click_tries = 0;
            demo.click_wait = 14;
            return;
        }
        if demo.click_tries >= 8 {
            println!(
                "CLICK {} intent ({x:.2},{y:.2}) NOT RECEIVED after 8 attempts (window key = {})",
                demo.click_index,
                sidebar.window_is_key()
            );
            demo.click_index += 1;
            demo.click_tries = 0;
            demo.click_wait = 14;
            return;
        }
        cx.activate(true);
        demo.click_tries += 1;
        click_at(window_to_global(sidebar, x, y));
        demo.click_wait = 14;
        return;
    }

    // Put the sidebar into state only AppKit knows about — selection, a collapsed group, a
    // scroll offset — then re-render hard with `update()` on every frame and read all three
    // back. A `reloadData` would have lost them; the `appkit_writes` delta must be 0.
    if env_flag("NG_STATE") && demo.row_click_index >= demo.row_click_rows.len() {
        demo.state_frames += 1;
        let n = demo.state_frames;
        if n == 5 {
            sidebar.set_expanded("g-mailboxes", false);
            sidebar.scroll_to(96.);
            println!("--- collapsed g-mailboxes, scrolled to 96 ---");
        }
        if n == 12 || n == 400 {
            println!(
                "STATE@{n:<4} selected={:?} g-mailboxes expanded={} g-smart expanded={} g-tags expanded={} scroll={:.2} rows={} writes={} renders={}",
                sidebar.selected(),
                sidebar.is_expanded("g-mailboxes"),
                sidebar.is_expanded("g-smart"),
                sidebar.is_expanded("g-tags"),
                sidebar.scroll_offset(),
                sidebar.probe().rows,
                sidebar.appkit_writes(),
                demo.frames,
            );
            if n == 12 {
                demo.writes_at_mark = sidebar.appkit_writes();
                demo.renders_at_mark = demo.frames;
            }
            if n == 400 {
                println!(
                    "--- {} GPUI renders later: appkit_writes delta = {} ---",
                    demo.frames - demo.renders_at_mark,
                    sidebar.appkit_writes() - demo.writes_at_mark
                );
            }
        }
    }

    if demo.row_click_index < demo.row_click_rows.len() {
        if demo.click_wait > 0 {
            demo.click_wait -= 1;
            return;
        }
        let row = demo.row_click_rows[demo.row_click_index];
        let expect = demo.row_click_expect[demo.row_click_index].clone();
        if sidebar.selected().map(|s| s.to_string()).as_deref() == Some(expect.as_str()) {
            println!(
                "ROWCLICK row {row} -> sidebar.selected()={:?} gpui state={:?} after {} attempt(s)",
                sidebar.selected(),
                demo.selected,
                demo.row_click_tries
            );
            demo.row_click_index += 1;
            demo.row_click_tries = 0;
            demo.click_wait = 30;
            return;
        }
        if demo.row_click_tries >= 8 {
            println!(
                "ROWCLICK row {row} FAILED after 8 attempts (window key = {}, selected = {:?})",
                sidebar.window_is_key(),
                sidebar.selected()
            );
            demo.row_click_index += 1;
            demo.row_click_tries = 0;
            demo.click_wait = 30;
            return;
        }
        let Some(at) = sidebar.row_point(row) else {
            println!("ROWCLICK row {row} not laid out");
            demo.row_click_index += 1;
            return;
        };
        cx.activate(true);
        demo.row_click_tries += 1;
        click_at(window_to_global(sidebar, at.x, at.y));
        demo.click_wait = 24;
        return;
    }
}

/// GPUI-logical window-content point -> global CGEvent point (top-left origin).
fn window_to_global(sidebar: &NativeSidebar, x: f64, y: f64) -> CGPoint {
    let frame = sidebar.window_frame();
    let flipped_top = screen_height() - (frame.origin.y + frame.size.height);
    CGPoint {
        x: frame.origin.x + x,
        y: flipped_top + y,
    }
}

fn main() {
    let opaque = env_flag("NG_OPAQUE");
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(200.), px(160.)),
            size: size(px(WIN_W), px(WIN_H)),
        })),
        ..window_options()
    };
    if opaque {
        options.window_background = gpui::WindowBackgroundAppearance::Opaque;
    }
    println!(
        "pid {} background={:?}",
        std::process::id(),
        options.window_background
    );

    Application::new().run(move |cx: &mut App| {
        cx.open_window(options, |_, cx| {
            cx.new(|_| Demo {
                installed: false,
                sidebar: None,
                selected: None,
                selections: Vec::new(),
                frames: 0,
                last_click: None,
                clicks: Vec::new(),
                last_diff: SidebarDiff::default(),
                extra: false,
                renamed: false,
                reordered: false,
                quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
                t0: Instant::now(),
                click_points: env::var("NG_CLICK")
                    .map(|spec| {
                        spec.split(';')
                            .filter_map(|p| {
                                let mut it = p.split(',');
                                Some((
                                    it.next()?.trim().parse().ok()?,
                                    it.next()?.trim().parse().ok()?,
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                click_index: 0,
                click_tries: 0,
                click_seen: 0,
                click_wait: 45,
                row_click_rows: env::var("NG_ROWCLICK")
                    .map(|spec| {
                        spec.split(',')
                            .filter_map(|r| r.split(':').next()?.trim().parse().ok())
                            .collect()
                    })
                    .unwrap_or_default(),
                row_click_expect: env::var("NG_ROWCLICK")
                    .map(|spec| {
                        spec.split(',')
                            .map(|r| r.split(':').nth(1).unwrap_or("").trim().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                row_click_index: 0,
                row_click_tries: 0,
                state_frames: 0,
                writes_at_mark: 0,
                renders_at_mark: 0,
                backdrop: None,
                edge: env_flag("NG_EDGE").then(Edge::default),
            })
        })
        .unwrap();
        cx.activate(true);
    });
}
