//! What a drag carries and where a drop lands: Finder's move, Option-copy and spring-loading.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use finder_fs::ops::Op;
use gpui::{
    Context, Div, DragMoveEvent, Hsla, IntoElement, Render, SharedString, Stateful, WeakEntity,
    Window, div, prelude::*, px,
};

use crate::app::FinderDemo;
use crate::ops::After;
use crate::view::COLUMN_VIEW;
use crate::views::list::TWIST_W;

/// What the springing delay preference adds to, and the delay Finder treats as none at all:
/// below it a hold waits a flash longer, not shorter.
const SPRING_FLOOR: f32 = 0.2;
const SPRING_NONE: f32 = 0.15;

/// One step of the flash a held folder gives before it springs, and the steps it takes: the row
/// goes dark, plain, dark, and then the folder opens.
const FLASH: Duration = Duration::from_millis(100);
const FLASHES: u32 = 3;

/// What a drop highlight's lightness falls to while the folder flashes before it springs.
const FLASH_DIM: f32 = 0.83;

/// What a row drag carries: the row the pointer grabbed, standing for the whole selection.
#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}

/// The row a drag grabbed, whichever view it started in.
pub(crate) trait Grabbed: 'static {
    fn key(&self) -> &SharedString;
}

impl Grabbed for RowDrag {
    fn key(&self) -> &SharedString {
        &self.key
    }
}

/// Whether a drag carries this row, and so never targets it: Finder drops nothing onto itself,
/// nor into anything it is carrying.
pub(crate) fn carries<D: Grabbed>(
    selected: &HashSet<SharedString>,
    grab: &D,
    key: &SharedString,
) -> bool {
    let under = |held: &SharedString| Path::new(key.as_ref()).starts_with(held.as_ref());
    match selected.contains(grab.key()) {
        true => selected.iter().any(under),
        false => under(grab.key()),
    }
}

/// How a drop target lights up: Finder outlines a whole column, and a view that draws the
/// highlight itself, from `drop_row`, needs no style of its own.
#[derive(Clone, Copy)]
pub(crate) enum Lit {
    Drawn,
    Outline(Hsla),
}

/// A drop highlight mid-flash: Finder darkens the fill rather than dropping it.
pub(crate) fn flashed(fill: Hsla) -> Hsla {
    Hsla {
        l: fill.l * FLASH_DIM,
        ..fill
    }
}

/// A folder under a drag springs open where the pointer rests on it: `band` is how far into the
/// row the disclosure triangle ends and how far the name beside it reaches, the strip Finder
/// treats as the row. A hold over the triangle discloses in place instead of opening.
pub(crate) fn drop_hover<D: Grabbed>(
    target: Stateful<Div>,
    key: SharedString,
    into: &'static str,
    band: Option<(f32, f32)>,
    carried: Arc<HashSet<SharedString>>,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    target.on_drag_move(move |event: &DragMoveEvent<D>, _, cx| {
        let at = event.event.position;
        let over = (event.bounds.contains(&at) && !carries(&carried, event.drag(cx), &key))
            .then(|| match band {
                Some((twist, name)) => {
                    let twist = event.bounds.left() + px(twist);
                    (at.x > twist - px(TWIST_W) && at.x <= event.bounds.left() + px(name))
                        .then(|| at.x <= twist)
                }
                None => Some(false),
            })
            .flatten();
        this.update(cx, |demo: &mut FinderDemo, cx| {
            demo.hover_folder(&key, into, over, cx);
        })
        .ok();
    })
}

/// A folder takes the rows dropped on it, lighting up while they are over it. A target that
/// lights from `drop_row` takes the drop only where it is lit.
pub(crate) fn drop_lands<D: Grabbed>(
    target: Stateful<Div>,
    key: SharedString,
    into: &'static str,
    carried: Arc<HashSet<SharedString>>,
    lit: Lit,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    target
        .drag_over::<D>({
            let carried = carried.clone();
            let key = key.clone();
            move |style, grab: &D, _, _| match (carries(&carried, grab, &key), lit) {
                (true, _) | (false, Lit::Drawn) => style,
                (false, Lit::Outline(edge)) => style.border_color(edge),
            }
        })
        .on_drop({
            let this = this.clone();
            let carried = carried.clone();
            let key = key.clone();
            move |grab: &D, window: &mut Window, cx| {
                if carries(&carried, grab, &key) {
                    return;
                }
                let grabbed = grab.key().clone();
                // Finder copies instead of moving while Option is held.
                let copy = window.modifiers().alt;
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    if matches!(lit, Lit::Drawn) && demo.drop_row() != Some(&key) {
                        return;
                    }
                    let keys = demo.drag_keys(&grabbed);
                    demo.drop_into(into, &keys, copy, cx);
                    cx.notify();
                })
                .ok();
            }
        })
}

/// A folder row: it takes the drop, and springs open under a drag that rests on it.
pub(crate) fn drop_target<D: Grabbed>(
    target: Stateful<Div>,
    key: SharedString,
    into: &'static str,
    band: Option<(f32, f32)>,
    carried: Arc<HashSet<SharedString>>,
    lit: Lit,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    let target = drop_lands::<D>(
        target,
        key.clone(),
        into,
        carried.clone(),
        lit,
        this.clone(),
    );
    drop_hover::<D>(target, key, into, band, carried, this)
}

/// A folder held under a drag: the row it is, where it opens, and when the hold began.
pub(crate) struct Spring {
    pub(crate) key: SharedString,
    into: &'static str,
    /// Whether the hold is over the row's twist, which discloses in place instead of opening.
    twist: bool,
    since: Instant,
    /// Set by Space, which springs whatever the springing delay is, and even where it is off.
    forced: bool,
    /// Set once the hold has sprung, so a pointer that stays put springs the row only once.
    spent: bool,
}

impl Spring {
    /// What this hold waits before it flashes, or `None` where it can never spring at all.
    fn delay(&self) -> Option<Duration> {
        match (self.spent, self.forced) {
            (true, _) => None,
            (_, true) => Some(Duration::ZERO),
            _ => springing_delay(),
        }
    }

    /// Which flash the hold has reached, `None` while it is still waiting the delay out.
    fn flash(&self) -> Option<u32> {
        let held = self.since.elapsed().checked_sub(self.delay()?)?;
        Some((held.as_millis() / FLASH.as_millis()) as u32)
    }
}

/// macOS's springing preferences, read once: how long a folder waits, or `None` where the user
/// turned springing off.
fn springing_delay() -> Option<Duration> {
    static DELAY: OnceLock<Option<Duration>> = OnceLock::new();
    *DELAY.get_or_init(|| {
        if global_default("com.apple.springing.enabled").as_deref() == Some("0") {
            return None;
        }
        let delay: f32 = global_default("com.apple.springing.delay")
            .and_then(|delay| delay.parse().ok())
            .unwrap_or(0.5);
        let held = match delay < SPRING_NONE {
            true => SPRING_FLOOR + delay + FLASH.as_secs_f32() * FLASHES as f32,
            false => SPRING_FLOOR + delay,
        };
        Some(Duration::from_secs_f32(held))
    })
}

fn global_default(key: &str) -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", key])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(out.stdout.trim_ascii()).into_owned())
}

pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl FinderDemo {
    /// Finder's spring-loading: a folder held under a drag flashes twice, then opens on its own.
    /// Leaving the row before it does cancels the hold.
    pub(crate) fn hover_folder(
        &mut self,
        key: &SharedString,
        into: &'static str,
        over: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let hold = self.spring.as_ref().filter(|spring| spring.key == *key);
        let (held, spent) = (hold.is_some(), hold.is_some_and(|spring| spring.spent));
        match (over, held, spent) {
            // A sprung row keeps its hold through the relayout its own disclosure causes.
            (_, true, true) => return,
            (Some(twist), true, _) => {
                if let Some(spring) = &mut self.spring {
                    spring.twist = twist;
                }
                return;
            }
            (Some(twist), false, _) => {
                self.spring = Some(Spring {
                    key: key.clone(),
                    into,
                    twist,
                    since: Instant::now(),
                    forced: false,
                    spent: false,
                })
            }
            (None, true, _) => self.spring = None,
            (None, false, _) => return,
        }
        cx.notify();
    }

    /// A hold, one frame on: it flashes while it waits, then opens the folder it rests on. Over
    /// the twist that is a disclosure in place, anywhere else the window walks in.
    pub(crate) fn spring_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spring.is_some() && !cx.has_active_drag() {
            self.spring = None;
        }
        let Some(spring) = &self.spring else {
            return;
        };
        if spring.delay().is_none() {
            return;
        }
        if spring.flash().is_none_or(|flash| flash < FLASHES) {
            window.request_animation_frame();
            return;
        }
        let Some(spring) = &mut self.spring else {
            return;
        };
        spring.spent = true;
        let (key, into, twist) = (spring.key.clone(), spring.into, spring.twist);
        match twist {
            true => {
                self.disclose(&key, true);
                cx.notify();
            }
            false if self.view == COLUMN_VIEW => self.column_open(&key, window, cx),
            false => self.go(into, window, cx),
        }
    }

    /// The fill a folder the drag rests on draws with, where the view lights it from state
    /// rather than from a hover style: dark part-way through the flash, plain otherwise.
    pub(crate) fn drop_fill(&self, key: &SharedString, fill: Hsla) -> Option<Hsla> {
        let hold = self.spring.as_ref()?;
        (hold.key == *key).then(|| match self.spring_flash() == Some(key) {
            true => flashed(fill),
            false => fill,
        })
    }

    /// The folder the drag rests on this frame, for a view that draws its highlight itself.
    pub(crate) fn drop_row(&self) -> Option<&SharedString> {
        Some(&self.spring.as_ref()?.key)
    }

    /// The row whose drop highlight is dark this frame, part-way through its flash.
    pub(crate) fn spring_flash(&self) -> Option<&SharedString> {
        let spring = self.spring.as_ref()?;
        (spring.flash()? % 2 == 0).then_some(&spring.key)
    }

    /// Finder's Space: the folder under the drag springs at once.
    pub(crate) fn spring_now(&mut self, cx: &mut Context<Self>) {
        if let Some(spring) = &mut self.spring {
            spring.since = Instant::now();
            spring.forced = true;
            cx.notify();
        }
    }

    /// Finder's Escape: the drag is called off where it stands, and nothing is dropped.
    pub(crate) fn cancel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !cx.stop_active_drag(window) {
            return false;
        }
        self.spring = None;
        cx.notify();
        true
    }

    /// A row drag reaching the sidebar becomes an AppKit file drag: the sidebar is an NSView.
    pub(crate) fn drag_to_sidebar(
        &mut self,
        event: &DragMoveEvent<RowDrag>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.event.position.x >= self.metrics().left {
            return;
        }
        let paths: Vec<PathBuf> = self
            .drag_keys(&event.drag(cx).key)
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        if native_gpui::sidebar::drag_files(window, &paths) {
            cx.stop_active_drag(window);
        }
    }

    /// The rows a drag carries: the whole selection when the grabbed row is in it.
    pub(crate) fn drag_keys(&self, grabbed: &SharedString) -> Vec<SharedString> {
        match self.selected.contains(grabbed) {
            true => self.selected.iter().cloned().collect(),
            false => vec![grabbed.clone()],
        }
    }

    /// Finder's drop: the dragged rows move into `into`, or copy there while Option is held.
    pub(crate) fn drop_into(
        &mut self,
        into: &'static str,
        keys: &[SharedString],
        copy: bool,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<PathBuf> = keys
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .filter(|path| path.parent() != Some(Path::new(into)))
            .collect();
        if items.is_empty() {
            return;
        }
        let into = PathBuf::from(into);
        let op = match copy {
            true => Op::Copy { items, into },
            false => Op::Move { items, into },
        };
        self.run_op(op, After::Select, cx);
    }
}
