use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use finder_fs::ops::Op;
use gpui::{
    App, Context, Div, DragMoveEvent, ExternalPaths, Hsla, IntoElement, Pixels, Point, Render,
    SharedString, Stateful, WeakEntity, Window, div, prelude::*, px,
};
use objc2_foundation::{NSNumber, NSString, NSUserDefaults};

use crate::app::FinderDemo;
use crate::ops::After;
use crate::view::{COLUMN_VIEW, ICON_VIEW};
use crate::views::list::TWIST_W;

const SPRING_FLOOR: f32 = 0.2;
const SPRING_NONE: f32 = 0.15;
const SPRING_DELAY: f32 = 0.5;

const FLASH: Duration = Duration::from_millis(100);
const FLASHES: u32 = 3;
const FLASH_DIM: f32 = 0.83;

/// Finder ignores a drop until the press has travelled this far; gpui starts a drag at 2.
const DRAG_START: f64 = 7.;

#[derive(Clone)]
pub(crate) struct RowDrag {
    pub(crate) key: SharedString,
}

pub(crate) trait Grabbed: 'static {
    fn carries(&self, selected: &HashSet<SharedString>, key: &SharedString) -> bool;
    fn keys(&self, selected: &HashSet<SharedString>) -> Vec<SharedString>;
    fn wandered(&self, window: &Window, pressed_at: Point<Pixels>) -> bool;
}

pub(crate) trait KeyDrag: 'static {
    fn key(&self) -> &SharedString;
}

impl KeyDrag for RowDrag {
    fn key(&self) -> &SharedString {
        &self.key
    }
}

impl<D: KeyDrag> Grabbed for D {
    fn carries(&self, selected: &HashSet<SharedString>, key: &SharedString) -> bool {
        match selected.contains(self.key()) {
            true => selected.iter().any(|held| under(key, held)),
            false => under(key, self.key()),
        }
    }

    fn keys(&self, selected: &HashSet<SharedString>) -> Vec<SharedString> {
        match selected.contains(self.key()) {
            true => selected.iter().cloned().collect(),
            false => vec![self.key().clone()],
        }
    }

    fn wandered(&self, window: &Window, pressed_at: Point<Pixels>) -> bool {
        (window.mouse_position() - pressed_at).magnitude() >= DRAG_START
    }
}

impl Grabbed for ExternalPaths {
    fn carries(&self, _: &HashSet<SharedString>, key: &SharedString) -> bool {
        let key = Path::new(key.as_ref());
        self.paths().iter().any(|held| key.starts_with(held))
    }

    fn keys(&self, _: &HashSet<SharedString>) -> Vec<SharedString> {
        self.paths()
            .iter()
            .map(|path| SharedString::from(path.to_string_lossy().into_owned()))
            .collect()
    }

    fn wandered(&self, _: &Window, _: Point<Pixels>) -> bool {
        true
    }
}

fn under(key: &str, held: &str) -> bool {
    Path::new(key).starts_with(held)
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

fn carried(this: &WeakEntity<FinderDemo>, cx: &App) -> Arc<HashSet<SharedString>> {
    this.upgrade()
        .map(|demo| demo.read(cx).selected.snapshot())
        .unwrap_or_default()
}

pub(crate) fn drop_hover<D: Grabbed>(
    target: Stateful<Div>,
    key: SharedString,
    into: &'static str,
    band: Option<(f32, f32)>,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    target.on_drag_move(move |event: &DragMoveEvent<D>, _, cx| {
        let at = event.event.position;
        let over = (event.bounds.contains(&at)
            && !event.drag(cx).carries(&carried(&this, cx), &key))
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
    lit: Lit,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    target
        .drag_over::<D>({
            let key = key.clone();
            let this = this.clone();
            move |style, grab: &D, _, cx| match lit {
                Lit::Outline(edge) if !grab.carries(&carried(&this, cx), &key) => {
                    style.border_color(edge)
                }
                _ => style,
            }
        })
        .on_drop(move |grab: &D, window: &mut Window, cx| {
            let carried = carried(&this, cx);
            if grab.carries(&carried, &key) {
                return;
            }
            let keys = grab.keys(&carried);
            this.update(cx, |demo: &mut FinderDemo, cx| {
                if matches!(lit, Lit::Drawn) && demo.drop_row() != Some(&key) {
                    return;
                }
                if !grab.wandered(window, demo.pressed_at) {
                    return;
                }
                demo.drop_into(into, &keys, window, cx);
                cx.notify();
            })
            .ok();
        })
}

pub(crate) fn drop_target<D: Grabbed>(
    target: Stateful<Div>,
    key: SharedString,
    into: &'static str,
    band: Option<(f32, f32)>,
    lit: Lit,
    this: WeakEntity<FinderDemo>,
) -> Stateful<Div> {
    let target = drop_lands::<D>(target, key.clone(), into, lit, this.clone());
    drop_hover::<D>(target, key, into, band, this)
}

pub(crate) struct Spring {
    key: SharedString,
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
            .unwrap_or(SPRING_DELAY);
        let held = match delay < SPRING_NONE {
            true => SPRING_FLOOR + delay + FLASH.as_secs_f32() * FLASHES as f32,
            false => SPRING_FLOOR + delay,
        };
        // A user default: it can be negative or infinite.
        Duration::try_from_secs_f32(held)
            .ok()
            .or(Some(Duration::from_secs_f32(SPRING_FLOOR + SPRING_DELAY)))
    })
}

fn global_default(key: &str) -> Option<String> {
    let value = NSUserDefaults::standardUserDefaults().objectForKey(&NSString::from_str(key))?;
    match value.downcast::<NSNumber>() {
        Ok(number) => Some(number.stringValue().to_string()),
        Err(value) => Some(value.downcast::<NSString>().ok()?.to_string()),
    }
}

pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl FinderDemo {
    fn hover_folder(
        &mut self,
        key: &SharedString,
        into: &'static str,
        over: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let held = self.spring.as_mut().filter(|spring| spring.key == *key);
        match (over, held) {
            // A sprung row keeps its hold through the relayout its own disclosure causes.
            (_, Some(spring)) if spring.spent => return,
            (Some(twist), Some(spring)) => {
                spring.twist = twist;
                return;
            }
            (Some(twist), None) => {
                self.spring = Some(Spring {
                    key: key.clone(),
                    into,
                    twist,
                    since: Instant::now(),
                    forced: false,
                    spent: false,
                })
            }
            (None, Some(_)) => self.spring = None,
            (None, None) => return,
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

    pub(crate) fn hand_off_drag<D: Grabbed>(
        &mut self,
        event: &DragMoveEvent<D>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = event.event.position;
        let size = window.viewport_size();
        let outside = at.x < px(0.) || at.y < px(0.) || at.x >= size.width || at.y >= size.height;
        if at.x >= self.metrics().left && !outside {
            return;
        }
        let selected = self.selected.snapshot();
        let paths: Vec<PathBuf> = event
            .drag(cx)
            .keys(&selected)
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        if native_gpui::sidebar::drag_files(window, &paths) {
            cx.stop_active_drag(window);
        }
    }

    pub(crate) fn files_dropped(
        &mut self,
        dropped: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.mouse_position().x < self.metrics().left {
            return;
        }
        let into = self
            .spring
            .as_ref()
            .map_or_else(|| self.here(), |spring| spring.into);
        if into == crate::locations::AIRDROP {
            self.airdrop_paths(dropped.paths());
            return;
        }
        if crate::locations::is_location(into) {
            return;
        }
        let keys = dropped.keys(&self.selected.snapshot());
        if self.view == ICON_VIEW && into == self.here() {
            self.land_icons(&keys, window.mouse_position());
        }
        self.drop_into(into, &keys, window, cx);
        cx.notify();
    }

    pub(crate) fn drop_into(
        &mut self,
        into: &'static str,
        keys: &[SharedString],
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let held = window.modifiers();
        let into = PathBuf::from(into);
        // A web link dragged from a browser arrives as its URL, not as a file.
        let items: Vec<PathBuf> = keys
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .filter(|path| path.is_absolute() && path.parent() != Some(into.as_path()))
            .collect();
        let sorted = cx.background_executor().spawn({
            let into = into.clone();
            async move {
                items.into_iter().partition::<Vec<PathBuf>, _>(|path| {
                    held.alt || (!held.platform && !same_volume(path, &into))
                })
            }
        });
        cx.spawn(async move |this, cx| {
            let (copies, moves) = sorted.await;
            this.update(cx, |this, cx| {
                if !moves.is_empty() {
                    let into = into.clone();
                    this.run_op(Op::Move { items: moves, into }, After::Select, cx);
                }
                if !copies.is_empty() {
                    this.run_op(
                        Op::Copy {
                            items: copies,
                            into,
                        },
                        After::Select,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }
}

fn same_volume(item: &Path, into: &Path) -> bool {
    match (std::fs::symlink_metadata(item), std::fs::metadata(into)) {
        (Ok(item), Ok(into)) => item.dev() == into.dev(),
        _ => false,
    }
}
