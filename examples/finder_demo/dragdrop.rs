use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
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

const SPRING_FLOOR: f32 = 0.2;
const SPRING_NONE: f32 = 0.15;

const FLASH: Duration = Duration::from_millis(100);
const FLASHES: u32 = 3;

const FLASH_DIM: f32 = 0.83;

/// Finder lets a press wander about 7 pt before it drops anything; gpui's drag starts at 2.
const DRAG_START: f64 = 7.;

#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}

pub(crate) trait Grabbed: 'static {
    fn key(&self) -> &SharedString;
}

impl Grabbed for RowDrag {
    fn key(&self) -> &SharedString {
        &self.key
    }
}

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

#[derive(Clone, Copy)]
pub(crate) enum Lit {
    Drawn,
    Outline(Hsla),
}

pub(crate) fn flashed(fill: Hsla) -> Hsla {
    Hsla {
        l: fill.l * FLASH_DIM,
        ..fill
    }
}

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
                this.update(cx, |demo: &mut FinderDemo, cx| {
                    if matches!(lit, Lit::Drawn) && demo.drop_row() != Some(&key) {
                        return;
                    }
                    let keys = demo.drag_keys(&grabbed);
                    demo.drop_into(into, &keys, window, cx);
                    cx.notify();
                })
                .ok();
            }
        })
}

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

pub(crate) struct Spring {
    pub(crate) key: SharedString,
    into: &'static str,
    twist: bool,
    since: Instant,
    forced: bool,
    spent: bool,
}

impl Spring {
    fn delay(&self) -> Option<Duration> {
        match (self.spent, self.forced) {
            (true, _) => None,
            (_, true) => Some(Duration::ZERO),
            _ => springing_delay(),
        }
    }

    fn flash(&self) -> Option<u32> {
        let held = self.since.elapsed().checked_sub(self.delay()?)?;
        Some((held.as_millis() / FLASH.as_millis()) as u32)
    }
}

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

    pub(crate) fn spring_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spring.is_some() && !cx.has_active_drag() {
            self.spring = None;
        }
        let Some(spring) = &mut self.spring else {
            return;
        };
        if spring.delay().is_none() {
            return;
        }
        if spring.flash().is_none_or(|flash| flash < FLASHES) {
            window.request_animation_frame();
            return;
        }
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

    pub(crate) fn drop_fill(&self, key: &SharedString, fill: Hsla) -> Option<Hsla> {
        let hold = self.spring.as_ref()?;
        (hold.key == *key).then(|| match self.spring_flash() == Some(key) {
            true => flashed(fill),
            false => fill,
        })
    }

    pub(crate) fn drop_row(&self) -> Option<&SharedString> {
        Some(&self.spring.as_ref()?.key)
    }

    pub(crate) fn spring_flash(&self) -> Option<&SharedString> {
        let spring = self.spring.as_ref()?;
        (spring.flash()? % 2 == 0).then_some(&spring.key)
    }

    pub(crate) fn spring_now(&mut self, cx: &mut Context<Self>) {
        if let Some(spring) = &mut self.spring {
            spring.since = Instant::now();
            spring.forced = true;
            cx.notify();
        }
    }

    pub(crate) fn cancel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !cx.stop_active_drag(window) {
            return false;
        }
        self.spring = None;
        cx.notify();
        true
    }

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

    pub(crate) fn drag_keys(&self, grabbed: &SharedString) -> Vec<SharedString> {
        match self.selected.contains(grabbed) {
            true => self.selected.iter().cloned().collect(),
            false => vec![grabbed.clone()],
        }
    }

    /// Finder moves within a volume and copies onto another; Option always copies, Command
    /// always moves.
    pub(crate) fn drop_into(
        &mut self,
        into: &'static str,
        keys: &[SharedString],
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if (window.mouse_position() - self.pressed_at).magnitude() < DRAG_START {
            return;
        }
        let held = window.modifiers();
        let into = PathBuf::from(into);
        let (copies, moves): (Vec<PathBuf>, Vec<PathBuf>) = keys
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .filter(|path| path.parent() != Some(into.as_path()))
            .partition(|path| held.alt || (!held.platform && !same_volume(path, &into)));
        if !moves.is_empty() {
            let op = Op::Move {
                items: moves,
                into: into.clone(),
            };
            self.run_op(op, After::Select, cx);
        }
        if !copies.is_empty() {
            self.run_op(
                Op::Copy {
                    items: copies,
                    into,
                },
                After::Select,
                cx,
            );
        }
    }
}

/// A side that can't be read counts as another volume, so the originals stay where they are.
fn same_volume(item: &Path, into: &Path) -> bool {
    match (std::fs::symlink_metadata(item), std::fs::metadata(into)) {
        (Ok(item), Ok(into)) => item.dev() == into.dev(),
        _ => false,
    }
}
