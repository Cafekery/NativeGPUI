use std::env;
use std::ffi::c_void;
use std::thread::sleep;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, Div, MouseButton, MouseDownEvent, Render, SharedString, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, rgba, size,
};
use native_gpui::sidebar::{
    NativeSidebar, PaneAlign, SidebarDiff, SidebarItem, SidebarMetrics, SidebarOptions,
    sidebar_group, sidebar_item,
};
use native_gpui::{Layer, NativeHost, native_root, window_options};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

type Rect = (f32, f32, f32, f32);

const WIN_W: f32 = 1000.;
const WIN_H: f32 = 640.;
const PROBE_ABOVE: Rect = (500., 24., 64., 64.);
const PROBE_BELOW: Rect = (500., 108., 168., 96.);
const COMB: f32 = 47.;

const EDGE_Y0: f64 = 386.;
const EDGE_DY: f64 = 16.;
const EDGE_STRIP: [f64; 5] = [1., 2., 3., 5., 12.];
// Fixed in GPUI-logical space, not derived from the gutter: it must not move while the pane does.
const EDGE_ANIM: Rect = (690., 292., 22., 22.);
const EDGE_ANIM_AIM: (f64, f64) = (700., 303.);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        position: NSPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGWarpMouseCursorPosition(point: NSPoint) -> i32;
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> NSPoint;
    fn CFRelease(cf: *const c_void);
}

const TAP_HID: u32 = 0;
const EV_MOUSE_MOVED: u32 = 5;
const EV_LEFT_DOWN: u32 = 1;
const EV_LEFT_UP: u32 = 2;

fn pointer_now() -> NSPoint {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        let at = CGEventGetLocation(event);
        CFRelease(event);
        at
    }
}

fn post(kind: u32, at: NSPoint) {
    unsafe {
        let event = CGEventCreateMouseEvent(std::ptr::null(), kind, at, 0);
        CGEventPost(TAP_HID, event);
        CFRelease(event);
    }
}

/// `CGWarpMouseCursorPosition` posts no event of its own, so the move has to be posted too.
fn warp(to: NSPoint) {
    unsafe { CGWarpMouseCursorPosition(to) };
    post(EV_MOUSE_MOVED, to);
}

fn synth_click(at: NSPoint, home: NSPoint, settle_ms: u64, step_ms: u64) {
    warp(at);
    sleep(Duration::from_millis(settle_ms));
    post(EV_LEFT_DOWN, at);
    sleep(Duration::from_millis(step_ms));
    post(EV_LEFT_UP, at);
    sleep(Duration::from_millis(step_ms));
    warp(home);
}

fn click_at(global: NSPoint) {
    let was = pointer_now();
    synth_click(global, was, 40, 30);
    println!(
        "CLICK posted at global ({:.1},{:.1}); pointer restored to ({:.1},{:.1})",
        global.x, global.y, was.x, was.y
    );
}

/// Posted off the main thread, so the animation the click is aimed into keeps running.
fn click_async(global: NSPoint, delay_ms: u64) {
    std::thread::spawn(move || {
        sleep(Duration::from_millis(delay_ms));
        synth_click(global, pointer_now(), 10, 10);
    });
}

fn window_to_global(sidebar: &NativeSidebar, x: f64, y: f64) -> NSPoint {
    let frame = sidebar.window_frame();
    let top = sidebar.screen_height() - (frame.origin.y + frame.size.height);
    NSPoint::new(frame.origin.x + x, top + y)
}

fn draw_origin(sidebar: &NativeSidebar) -> (f64, f64) {
    let pane = sidebar.pane_rect();
    let gpui = sidebar.gpui_frame();
    let cv = sidebar.content_view_size();
    (
        pane.origin.x + gpui.origin.x,
        cv.height - (pane.origin.y + gpui.origin.y + gpui.size.height),
    )
}

fn same_size(a: NSSize, b: NSSize) -> bool {
    (a.width - b.width).abs() < 0.5 && (a.height - b.height).abs() < 0.5
}

#[derive(Clone, Copy)]
struct EdgeTarget {
    label: &'static str,
    rect: Rect,
    aim: (f64, f64),
    dx: Option<f64>,
}

fn edge_targets(sidebar: &NativeSidebar) -> Vec<EdgeTarget> {
    let left = f64::from(f32::from(sidebar.metrics().left));
    let w = sidebar.content_view_size().width;
    let inside = [
        ("edge+1", 1.),
        ("edge+2", 2.),
        ("edge+3", 3.),
        ("edge+4", 4.),
        ("edge+6", 6.),
        ("edge+8", 8.),
    ]
    .map(|(label, dx)| (label, left + dx, Some(dx)));
    let right = [
        ("right-1", 1.),
        ("right-2", 2.),
        ("right-3", 3.),
        ("right-4", 4.),
        ("right-6", 6.),
        ("right-8", 8.),
        ("right-16", 16.),
    ]
    .map(|(label, dx)| (label, w - dx, None));
    inside
        .into_iter()
        .chain([("mid", left + (w - left) / 2., None)])
        .chain(right)
        .enumerate()
        .map(|(k, (label, ax, dx))| {
            let ay = EDGE_Y0 + k as f64 * EDGE_DY;
            let rx = (ax - 1.).clamp(left, (w - 20.).max(left));
            EdgeTarget {
                label,
                rect: (rx as f32, (ay - 7.) as f32, 20., 14.),
                aim: (ax, ay),
                dx,
            }
        })
        .collect()
}

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
    sel: Option<SharedString>,
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
    recv: Option<Recv>,
    hit: Option<usize>,
    anim_hit: bool,
    anim_frames: u64,
    rows: Vec<EdgeRow>,
    strip_i: usize,
    strip_tries: u32,
    strip: Vec<(f64, bool)>,
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
        same_size(g.size, cv),
        draw.0,
        draw.1,
        sidebar.probe().divider_thickness,
        sidebar.align_count(),
        sidebar.frame_corrections(),
    );
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

fn edge_pass(edge: &mut Edge, sidebar: &NativeSidebar) -> bool {
    let Some(&t) = edge.targets.get(edge.ti) else {
        edge.ti = 0;
        return true;
    };
    let draw = draw_origin(sidebar);
    let cv = sidebar.content_view_size();
    let posted = (draw.0 + t.aim.0, draw.1 + t.aim.1);
    let onscreen = posted.0 >= 0. && posted.0 < cv.width && posted.1 >= 0. && posted.1 < cv.height;
    let received = edge.recv.take();
    if received.is_none() && onscreen && edge.tries < 6 {
        edge.tries += 1;
        click_at(window_to_global(sidebar, posted.0, posted.1));
        edge.wait = 10;
        return false;
    }

    let sel = sidebar.selected();
    let (got, hit, tries, wait) = match received {
        Some(got) => {
            let hit = edge.hit.take() == Some(edge.ti);
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
            (Some((got.x, got.y)), hit, edge.tries, 8)
        }
        None if !onscreen => {
            println!(
                "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) posted ({:7.2},{:7.2}) OFF-WINDOW (contentView {:.0}x{:.0}): the target is clipped away, so there is nothing to click",
                edge.state, t.label, t.aim.0, t.aim.1, posted.0, posted.1, cv.width, cv.height,
            );
            (None, false, 0, 4)
        }
        None => {
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
            (None, false, edge.tries, 8)
        }
    };
    edge.rows.push(EdgeRow {
        state: edge.state,
        label: t.label,
        aim: t.aim,
        posted,
        got,
        hit,
        onscreen,
        tries,
        sel,
        dx: t.dx,
    });
    edge.ti += 1;
    edge.tries = 0;
    edge.wait = wait;
    false
}

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
    if let Some(got) = edge.recv.take() {
        let label = "anim@700";
        let hit = std::mem::take(&mut edge.anim_hit);
        println!(
            "EDGE {:<12} {:<8} aim ({:7.2},{:7.2}) got ({:7.2},{:7.2}) err ({:+6.2},{:+6.2}) hit {:<5} drawOrigin-at-receipt ({:+.2},{:+.2}) paneX-at-receipt {:7.2} aligns {} corr {}",
            edge.state,
            label,
            EDGE_ANIM_AIM.0,
            EDGE_ANIM_AIM.1,
            got.x,
            got.y,
            got.x as f64 - EDGE_ANIM_AIM.0,
            got.y as f64 - EDGE_ANIM_AIM.1,
            hit,
            got.draw.0,
            got.draw.1,
            got.pane_x,
            got.aligns,
            got.corrections,
        );
        edge.rows.push(EdgeRow {
            state: edge.state,
            label,
            aim: EDGE_ANIM_AIM,
            posted: EDGE_ANIM_AIM,
            got: Some((got.x, got.y)),
            hit,
            onscreen: true,
            tries: 1,
            sel: sidebar.selected(),
            dx: None,
        });
    }
    edge.anim_frames >= 42
}

fn pair<T: std::fmt::Display>((x, y): (T, T)) -> String {
    format!("({:7.2},{:7.2})", x, y)
}

fn edge_table(edge: &Edge, sidebar: &NativeSidebar) {
    println!("\n=== EDGE SWEEP TABLE (arm {}) ===", edge_arm());
    println!(
        "{:<12} {:<9} {:>17} {:>17} {:>17} {:>16} {:<5} {:>4}  note",
        "state",
        "target",
        "aimed logical",
        "posted window",
        "received logical",
        "error (pt)",
        "hit",
        "att"
    );
    let mut worst = 0f64;
    let mut received = 0usize;
    let mut hits = 0usize;
    for r in &edge.rows {
        let (got, err, note) = match r.got {
            Some(got) => {
                received += 1;
                hits += usize::from(r.hit);
                let ex = got.0 as f64 - r.aim.0;
                let ey = got.1 as f64 - r.aim.1;
                worst = worst.max(ex.abs()).max(ey.abs());
                (
                    pair(got),
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
            pair(r.aim),
            pair(r.posted),
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

    let mut states: Vec<&str> = edge.rows.iter().map(|r| r.state).collect();
    states.dedup();
    for state in states {
        let probed: Vec<(f64, bool)> = edge
            .rows
            .iter()
            .filter(|r| r.state == state)
            .filter_map(|r| Some((r.dx?, r.got.is_some())))
            .collect();
        if probed.is_empty() {
            continue;
        }
        let first = probed.iter().filter(|p| p.1).map(|p| p.0).reduce(f64::min);
        let swallowed: Vec<f64> = probed.iter().filter(|p| !p.1).map(|p| p.0).collect();
        println!(
            "LEFT-EDGE PROFILE {state:<12} first offset that reached GPUI = {} pt inside the pane edge; swallowed at {swallowed:?} pt",
            first.map_or_else(|| "none of them".to_string(), |first| format!("{first:.0}")),
        );
    }
    if !edge.strip.is_empty() {
        let offsets = |answered: bool| -> Vec<f64> {
            edge.strip
                .iter()
                .filter(|s| s.1 == answered)
                .map(|s| s.0)
                .collect()
        };
        let (answered, eaten) = (offsets(true), offsets(false));
        println!(
            "LEFT-STRIP PROBE (sidebar expanded, aimed at row 1 inside the sidebar strip, detector = AppKit selection): the outline view answered at {answered:?} pt inside the window's left edge and nothing arrived at {eaten:?} pt. Read with EDGE-ROW1: rows start 8pt in, so this bounds a band dead to BOTH hit paths and does not isolate a window frame border"
        );
    }
    let mut sels: Vec<&str> = edge
        .rows
        .iter()
        .map(|r| r.sel.as_deref().unwrap_or("none"))
        .collect();
    sels.sort_unstable();
    sels.dedup();
    println!(
        "SIDEBAR SELECTION observed across the sweep: {sels:?} — a click that reached the outline view instead of GPUI would have changed it"
    );
    edge_readout("final", sidebar);
}

fn fill_arm() -> bool {
    env::var("NG_ARM").as_deref() == Ok("fill")
}

fn edge_arm() -> &'static str {
    if fill_arm() {
        "fill (broken control)"
    } else {
        "aligned (shipped)"
    }
}

fn print_targets(targets: &[EdgeTarget]) {
    for t in targets {
        println!(
            "  target {:<8} logical rect ({:.1},{:.1},{:.1}x{:.1}) aim ({:.2},{:.2})",
            t.label, t.rect.0, t.rect.1, t.rect.2, t.rect.3, t.aim.0, t.aim.1
        );
    }
}

fn edge_retarget(edge: &mut Edge, sidebar: &NativeSidebar, when: &str, state: &'static str) {
    edge_readout(when, sidebar);
    edge.state = state;
    edge.targets = edge_targets(sidebar);
    edge.wait = 10;
}

fn edge_strip(edge: &mut Edge, sidebar: &NativeSidebar) {
    let Some(&dx) = EDGE_STRIP.get(edge.strip_i) else {
        edge.phase = 13;
        return;
    };
    let answered = sidebar.selected().is_some_and(|s| s == "inbox");
    if !answered && edge.strip_tries < 3 {
        let Some(at) = sidebar.row_point(1) else {
            println!("EDGE-STRIP row 1 is not laid out; probe skipped");
            edge.phase = 13;
            return;
        };
        edge.strip_tries += 1;
        click_at(window_to_global(sidebar, dx, at.y));
        edge.wait = 14;
        return;
    }
    if answered {
        println!(
            "EDGE-STRIP {dx:>4.0} pt inside the window's left edge -> outline view selected `inbox` after {} attempt(s)",
            edge.strip_tries
        );
        sidebar.set_selected(None);
    } else {
        println!(
            "EDGE-STRIP {dx:>4.0} pt inside the window's left edge -> nothing reached the outline view in 3 attempts (window key {})",
            sidebar.window_is_key()
        );
    }
    edge.strip.push((dx, answered));
    edge.strip_i += 1;
    edge.strip_tries = 0;
    edge.wait = 12;
}

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
            print_targets(&edge.targets);
            // A concurrent `screencapture` loop silently stops synthetic clicks being delivered.
            if env_flag("NG_EDGE_HOLD") {
                println!("EDGE HOLD: targets painted, no input will be posted");
                edge.phase = 100;
            } else {
                edge.phase = 1;
                edge.wait = 30;
            }
        }
        1 | 5 | 8 | 11 => {
            if edge_pass(edge, sidebar) {
                edge.phase += 1;
            }
        }
        2 => {
            println!("--- resize to 1240x760 ---");
            window.resize(size(px(1240.), px(760.)));
            edge.phase = 3;
            edge.wait = 25;
        }
        3 => {
            edge_readout("resized 1240x760", sidebar);
            println!("--- resize back to {WIN_W:.0}x{WIN_H:.0} ---");
            window.resize(size(px(WIN_W), px(WIN_H)));
            edge.phase = 4;
            edge.wait = 25;
        }
        4 => {
            edge_retarget(edge, sidebar, "after resize", "after-resize");
            edge.phase = 5;
        }
        6 | 9 => {
            let draw = draw_origin(sidebar);
            let at = window_to_global(sidebar, draw.0 + EDGE_ANIM_AIM.0, draw.1 + EDGE_ANIM_AIM.1);
            let (x, y) = EDGE_ANIM_AIM;
            if edge.phase == 6 {
                println!(
                    "--- collapsing; async click queued at global ({:.1},{:.1}) for logical ({:.1},{:.1}) ---",
                    at.x, at.y, x, y
                );
                edge.state = "collapsing";
            } else {
                println!(
                    "--- expanding; async click queued at global ({:.1},{:.1}) for logical ({:.1},{:.1}) ---",
                    at.x, at.y, x, y
                );
                edge.state = "expanding";
            }
            edge.anim_frames = 0;
            click_async(at, 60);
            sidebar.toggle();
            edge.phase += 1;
        }
        7 => {
            if edge_anim(edge, sidebar) {
                edge_retarget(edge, sidebar, "after collapse", "collapsed");
                print_targets(&edge.targets);
                edge.phase = 8;
            }
        }
        10 => {
            if edge_anim(edge, sidebar) {
                edge_retarget(edge, sidebar, "after expand", "re-expanded");
                edge.phase = 11;
            }
        }
        12 => edge_strip(edge, sidebar),
        100 => {}
        _ => {
            edge_table(edge, sidebar);
            println!("EDGE SWEEP done");
            std::process::exit(0);
        }
    }
}

fn env_flag(key: &str) -> bool {
    env::var(key).is_ok_and(|v| v != "0")
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    env::var(key).ok()?.parse().ok()
}

struct Knobs {
    animate: bool,
    probe: bool,
    resize: bool,
    toggle: bool,
    toggle_every: Option<u64>,
    mutate: bool,
    state: bool,
    rows: usize,
    backdrop: Option<String>,
    host_probes: bool,
}

impl Knobs {
    fn from_env() -> Self {
        let probe = env_flag("NG_PROBE");
        let resize = env_flag("NG_RESIZE");
        let state = env_flag("NG_STATE");
        let toggle = env_flag("NG_TOGGLE");
        let mutate = env_flag("NG_MUTATE");
        let set = |key: &str| env::var(key).is_ok();
        Self {
            animate: env_flag("NG_CHURN")
                || probe
                || resize
                || state
                || toggle
                || mutate
                || set("NG_CLICK")
                || set("NG_ROWCLICK")
                || set("NG_QUIT")
                || env_flag("NG_EDGE"),
            probe,
            resize,
            toggle,
            toggle_every: env_parse("NG_TOGGLE_EVERY"),
            mutate,
            state,
            rows: env_parse("NG_ROWS").unwrap_or(0),
            backdrop: env::var("NG_BACKDROP").ok(),
            host_probes: !env_flag("NG_NOPROBES"),
        }
    }
}

struct Demo {
    knobs: Knobs,
    installed: bool,
    sidebar: Option<NativeSidebar>,
    selected: Option<SharedString>,
    selections: Vec<SharedString>,
    frames: u64,
    last_click: Option<(f32, f32)>,
    clicks: usize,
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
    edge: Option<Edge>,
}

impl Demo {
    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if env_flag("NG_NOSIDEBAR") {
            println!("NO SIDEBAR arm: gpui alone");
            return;
        }
        let align = if fill_arm() {
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
        let again = NativeSidebar::install(window, cx, SidebarOptions::default());
        println!(
            "idempotent: subviews of window contentView = {}, second install same handle = {}",
            sidebar.split_subview_count(),
            sidebar.window_number() == again.window_number()
        );

        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, _, cx| {
            println!("SELECT -> {id:?}");
            this.selections.extend(id.clone());
            this.selected = id.clone();
            cx.notify();
        }));
        let diff = sidebar.update(&self.items());
        let frame = sidebar.window_frame();
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
            same_size(gpui.size, content),
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

    fn items(&self) -> Vec<SidebarItem> {
        let drafts = sidebar_item("drafts", "Drafts").symbol("doc.text");
        let mut mailboxes = vec![
            sidebar_item("inbox", "Inbox").symbol("tray.full").badge(12),
            sidebar_item("sent", "Sent").symbol("paperplane"),
            if self.renamed {
                drafts.badge(3)
            } else {
                drafts
            },
        ];
        if self.reordered {
            mailboxes.swap(0, 2);
        }
        if self.extra {
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
        if !self.extra {
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
                .chain((0..self.knobs.rows).map(|i| {
                    sidebar_item(format!("pad-{i}"), format!("Tag {}", i + 1)).symbol("tag")
                })),
            ),
        ]
    }
}

/// The sidebar's material blurs what is behind the window, which a window capture cannot contain.
fn install_backdrop(sidebar: &NativeSidebar, white: bool) -> Retained<NSWindow> {
    let mtm = MainThreadMarker::new().expect("main thread");
    let f = sidebar.window_frame();
    let rect = NSRect::new(
        NSPoint::new(f.origin.x - 80., f.origin.y - 80.),
        NSSize::new(f.size.width + 160., f.size.height + 160.),
    );
    let win = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
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

fn host_probe(
    window: &mut Window,
    cx: &mut App,
    key: u64,
    layer: Layer,
    (x, y, w, h): Rect,
    (r, g, b): (f64, f64, f64),
) {
    let bounds = Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    };
    let mask = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(4000.), px(4000.)),
    };
    NativeHost::get(window, cx).place(key, layer, bounds, mask, move |mtm, _| {
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(1., 1.)),
        );
        view.setWantsLayer(true);
        if let Some(l) = view.layer() {
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.);
            l.setBackgroundColor(Some(&color.CGColor()));
        }
        view
    });
}

fn block((x, y, w, h): Rect) -> Div {
    div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.installed {
            self.installed = true;
            self.install(window, cx);
        }
        self.frames += 1;

        if let Some(sidebar) = self.sidebar.clone() {
            let diff = sidebar.update(&self.items());
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

        if self.knobs.animate {
            window.request_animation_frame();
        }
        if self.knobs.probe && self.frames % 30 == 1 {
            self.probe();
        }
        if self.quit_at.is_some_and(|limit| self.frames >= limit) {
            println!(
                "EXIT after {} frames, {:.2}s",
                self.frames,
                self.t0.elapsed().as_secs_f64()
            );
            std::process::exit(0);
        }

        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
        let left = f32::from(metrics.left);
        let top = f32::from(metrics.top);
        if self.backdrop.is_none() {
            if let (Some(kind), Some(sidebar)) = (&self.knobs.backdrop, &self.sidebar) {
                self.backdrop = Some(install_backdrop(sidebar, kind == "white"));
                println!("BACKDROP {kind} installed");
            }
        }
        if self.knobs.host_probes {
            for (key, layer, rect, color) in [
                (1, Layer::Above, PROBE_ABOVE, (1.0, 0.42, 0.0)),
                (2, Layer::Below, PROBE_BELOW, (0.0, 0.85, 0.35)),
            ] {
                let placed = (left + rect.0, top + rect.1, rect.2, rect.3);
                host_probe(window, cx, key, layer, placed, color);
            }
        }

        let selected = self.selected.as_deref().unwrap_or("—");
        let frames = self.frames;
        let swatch = match selected {
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
        let (hx, hy, hw, hh) = (
            left + PROBE_BELOW.0,
            top + PROBE_BELOW.1,
            PROBE_BELOW.2,
            PROBE_BELOW.3,
        );
        let panel = rgb(0x16191d);
        let caption = |text: String| {
            div()
                .text_size(px(13.))
                .text_color(rgb(0x9aa0a6))
                .child(text)
        };

        // Nothing paints over the sidebar strip: a GPUI background there hides its material.
        native_root(
            div()
                .relative()
                .size_full()
                // A hole over the Layer::Below probe, which composites under GPUI's own content.
                .children(
                    [
                        (left, 0., WIN_W - left, hy),
                        (left, hy + hh, WIN_W - left, WIN_H - hy - hh),
                        (left, hy, hx - left, hh),
                        (hx + hw, hy, WIN_W - hx - hw, hh),
                    ]
                    .map(|(x, y, w, h)| block((x, y, w.max(0.), h.max(0.))).bg(panel)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(0.))
                        .size_full()
                        .children((1..22).map(|k| {
                            block((k as f32 * COMB, 0., 1., WIN_H)).bg(rgba(0xffffff99))
                        })),
                )
                .child(block((left + 4., top + 240., 90., 44.)).bg(rgb(0xffcc00)))
                .children([(330., 0xffffff), (370., 0x000000)].map(|(dy, color)| {
                    block((left + 320., top + dy, 160., 40.)).bg(rgb(color))
                }))
                .child(
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
                        .child(caption(format!(
                            "gutter left {left:.1} top {top:.1} · frame {frames} · clicks received {}",
                            self.clicks
                        )))
                        .child(caption(format!(
                            "history: {}",
                            self.selections.join(" \u{2192} ")
                        ))),
                )
                .child(
                    block((left + 24., top + 140., 220., 72.))
                        .rounded(px(10.))
                        .bg(rgb(swatch)),
                )
                .when_some(self.last_click, |root, (x, y)| {
                    root.child(block((x - 7., y - 7., 14., 14.)).bg(rgb(0x00ff44)))
                })
                .when_some(self.edge.as_ref(), |root, edge| {
                    root.children(edge.targets.iter().enumerate().map(|(k, t)| {
                        block(t.rect).bg(rgb(0x00e5ff)).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, _| {
                                if let Some(edge) = this.edge.as_mut() {
                                    edge.hit = Some(k);
                                }
                            }),
                        )
                    }))
                    .child(block(EDGE_ANIM).bg(rgb(0xff2d95)).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, _| {
                            if let Some(edge) = this.edge.as_mut() {
                                edge.anim_hit = true;
                            }
                        }),
                    ))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                        println!("GPUI RECEIVED mouse down at ({x:.2}, {y:.2})");
                        this.last_click = Some((x, y));
                        this.clicks += 1;
                        // Read in the delivering dispatch: afterwards the pane has already moved.
                        if let (Some(edge), Some(sidebar)) = (this.edge.as_mut(), &this.sidebar) {
                            edge.recv = Some(Recv {
                                x,
                                y,
                                draw: draw_origin(sidebar),
                                pane_x: sidebar.pane_rect().origin.x,
                                aligns: sidebar.align_count(),
                                corrections: sidebar.frame_corrections(),
                            });
                        }
                        cx.notify();
                    }),
                ),
        )
    }
}

fn script(demo: &mut Demo, sidebar: &NativeSidebar, window: &mut Window, cx: &mut Context<Demo>) {
    if let Some(edge) = demo.edge.as_mut() {
        edge_step(edge, sidebar, window, cx);
        return;
    }
    let f = demo.frames;

    if demo.knobs.resize {
        let to = match f {
            60 => Some(size(px(1240.), px(760.))),
            160 => Some(size(px(760.), px(520.))),
            260 => Some(size(px(WIN_W), px(WIN_H))),
            _ => None,
        };
        if let Some(to) = to {
            println!(
                "--- resize to {:.0}x{:.0} ---",
                f32::from(to.width),
                f32::from(to.height)
            );
            window.resize(to);
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
                same_size(gpui.size, content),
                f32::from(sidebar.metrics().left),
            );
        }
    }

    if demo
        .knobs
        .toggle_every
        .is_some_and(|every| f > 40 && f.is_multiple_of(every))
    {
        sidebar.toggle();
    }

    if demo.knobs.toggle {
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
            let global_left = cx
                .try_global::<SidebarMetrics>()
                .map_or(-1., |m| f32::from(m.left));
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
                same_size(gpui.size, content),
            );
        }
    }

    if demo.knobs.mutate {
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

    // A non-key window silently eats synthetic clicks, so every attempt re-activates first.
    if let Some(&(x, y)) = demo.click_points.get(demo.click_index) {
        if demo.click_wait > 0 {
            demo.click_wait -= 1;
            return;
        }
        demo.click_wait = 14;
        if let Some(got) = demo.last_click.filter(|_| demo.clicks > demo.click_seen) {
            println!(
                "CLICK {} intent ({x:.2},{y:.2}) RECEIVED at ({:.2},{:.2}) err ({:+.2},{:+.2}) after {} attempt(s)",
                demo.click_index,
                got.0,
                got.1,
                got.0 as f64 - x,
                got.1 as f64 - y,
                demo.click_tries
            );
            demo.click_seen = demo.clicks;
        } else if demo.click_tries >= 8 {
            println!(
                "CLICK {} intent ({x:.2},{y:.2}) NOT RECEIVED after 8 attempts (window key = {})",
                demo.click_index,
                sidebar.window_is_key()
            );
        } else {
            cx.activate(true);
            demo.click_tries += 1;
            click_at(window_to_global(sidebar, x, y));
            return;
        }
        demo.click_index += 1;
        demo.click_tries = 0;
        return;
    }

    if demo.knobs.state && demo.row_click_index >= demo.row_click_rows.len() {
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
            } else {
                println!(
                    "--- {} GPUI renders later: appkit_writes delta = {} ---",
                    demo.frames - demo.renders_at_mark,
                    sidebar.appkit_writes() - demo.writes_at_mark
                );
            }
        }
    }

    if let Some(&row) = demo.row_click_rows.get(demo.row_click_index) {
        if demo.click_wait > 0 {
            demo.click_wait -= 1;
            return;
        }
        let expect = demo.row_click_expect[demo.row_click_index].as_str();
        if sidebar.selected().as_deref() == Some(expect) {
            println!(
                "ROWCLICK row {row} -> sidebar.selected()={:?} gpui state={:?} after {} attempt(s)",
                sidebar.selected(),
                demo.selected,
                demo.row_click_tries
            );
        } else if demo.row_click_tries >= 8 {
            println!(
                "ROWCLICK row {row} FAILED after 8 attempts (window key = {}, selected = {:?})",
                sidebar.window_is_key(),
                sidebar.selected()
            );
        } else {
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
        demo.row_click_index += 1;
        demo.row_click_tries = 0;
        demo.click_wait = 30;
    }
}

fn main() {
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(200.), px(160.)),
            size: size(px(WIN_W), px(WIN_H)),
        })),
        ..window_options()
    };
    if env_flag("NG_OPAQUE") {
        options.window_background = gpui::WindowBackgroundAppearance::Opaque;
    }
    println!(
        "pid {} background={:?}",
        std::process::id(),
        options.window_background
    );

    gpui_platform::application().run(move |cx: &mut App| {
        cx.open_window(options, |_, cx| {
            cx.new(|_| Demo {
                knobs: Knobs::from_env(),
                installed: false,
                sidebar: None,
                selected: None,
                selections: Vec::new(),
                frames: 0,
                last_click: None,
                clicks: 0,
                last_diff: SidebarDiff::default(),
                extra: false,
                renamed: false,
                reordered: false,
                quit_at: env_parse("NG_QUIT"),
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
