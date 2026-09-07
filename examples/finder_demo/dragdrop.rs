//! What a drag carries and where a drop lands: Finder's move, Option-copy and spring-loading.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use finder_fs::ops::Op;
use gpui::{Context, DragMoveEvent, IntoElement, Render, SharedString, Window, div};

use crate::app::FinderDemo;
use crate::ops::After;

/// What the springing delay preference adds to.
const SPRING_FLOOR: f32 = 0.5;

/// One step of the flash a held folder gives before it springs, and the steps it takes: the row
/// goes dark, plain, dark, and then the folder opens.
const FLASH: Duration = Duration::from_millis(100);
const FLASHES: u32 = 3;

/// What a row drag carries: the row the pointer grabbed, standing for the whole selection.
#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}

/// Whether a drag carries this row, and so never targets it: Finder drops nothing onto itself,
/// nor into anything it is carrying.
pub(crate) fn carries(
    selected: &HashSet<SharedString>,
    grab: &RowDrag,
    key: &SharedString,
) -> bool {
    let under = |held: &SharedString| Path::new(key.as_ref()).starts_with(held.as_ref());
    match selected.contains(&grab.key) {
        true => selected.iter().any(under),
        false => under(&grab.key),
    }
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
        Some(Duration::from_secs_f32(SPRING_FLOOR + delay))
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
            false => self.go(into, window, cx),
        }
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
