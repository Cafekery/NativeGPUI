use std::cell::Cell;
use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, EntityId, Pixels, Subscription, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    LabelAlign, native_label, native_root, remember_frame, titlebar_band, window_options,
};
use objc2::MainThreadMarker;

use crate::app::FinderDemo;
use crate::chrome::toolbar::{system_corners, titled_height};
use crate::menus::ShowProgressWindow;
use crate::ops::jobs::{ROW_H, progress_row};

const WIDTH: f32 = 404.;
const TICK: Duration = Duration::from_millis(100);
const FRAME: &str = "ng.finder.progress";

thread_local! {
    static OPEN: Cell<Option<WindowHandle<Progress>>> = const { Cell::new(None) };
}

pub(crate) fn shown() -> bool {
    OPEN.get().is_some()
}

pub(crate) fn toggle(_: &ShowProgressWindow, cx: &mut App) {
    match OPEN.get() {
        Some(open) => {
            open.update(cx, |_, window, _| window.remove_window()).ok();
        }
        None => open(true, cx),
    }
}

/// Opens for a job that runs a while, leaving the key window where it is.
pub(crate) fn reveal(cx: &mut App) {
    if !shown() {
        open(false, cx);
    }
}

fn open(focus: bool, cx: &mut App) {
    // Deferred: the first draw reads every Finder window, and callers are inside one's update.
    cx.defer(move |cx| {
        let Some(mtm) = MainThreadMarker::new().filter(|_| !shown()) else {
            return;
        };
        let tall = height(rows_shown(&demos(cx), cx), mtm);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(160.), px(160.)),
                    size: size(px(WIDTH), tall),
                })),
                focus,
                is_resizable: false,
                ..window_options()
            },
            |window, cx| {
                window.set_window_title("Copy");
                system_corners(window);
                remember_frame(window, FRAME);
                window.resize(size(px(WIDTH), tall));
                cx.new(|cx| {
                    cx.on_release(|_, _| OPEN.set(None)).detach();
                    Progress {
                        watched: HashSet::new(),
                        watching: Vec::new(),
                        tall,
                        had_rows: false,
                        ticking: false,
                    }
                })
            },
        );
        if let Ok(opened) = opened {
            OPEN.set(Some(opened));
        }
    });
}

fn demos(cx: &App) -> Vec<Entity<FinderDemo>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<FinderDemo>()?.entity(cx).ok())
        .collect()
}

fn rows_shown(demos: &[Entity<FinderDemo>], cx: &App) -> usize {
    demos
        .iter()
        .map(|demo| demo.read(cx).jobs.iter().filter(|job| job.shown()).count())
        .sum()
}

fn height(rows: usize, mtm: MainThreadMarker) -> Pixels {
    titled_height(rows.max(1) as f32 * ROW_H, mtm)
}

struct Progress {
    watched: HashSet<EntityId>,
    watching: Vec<Subscription>,
    tall: Pixels,
    /// Closes itself once its last row has gone, but not when opened with nothing in it.
    had_rows: bool,
    ticking: bool,
}

impl Progress {
    /// Reads the running jobs ten times a second, and stops once none is left.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let running = this.update(cx, |this, cx| {
                    let running = demos(cx).into_iter().fold(false, |running, demo| {
                        demo.update(cx, |demo, _| demo.poll_jobs()) || running
                    });
                    this.ticking = running;
                    cx.notify();
                    running
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

impl Render for Progress {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demos = demos(cx);
        for demo in &demos {
            if self.watched.insert(demo.entity_id()) {
                self.watching.push(cx.observe(demo, |_, _, cx| cx.notify()));
            }
        }
        let mut running = false;
        let mut rows: Vec<AnyElement> = Vec::new();
        for demo in &demos {
            for job in demo.read(cx).jobs.iter().filter(|job| job.shown()) {
                running |= job.running();
                rows.push(progress_row(job, demo));
            }
        }
        if rows.is_empty() && self.had_rows {
            cx.defer_in(window, |_, window, _| window.remove_window());
        }
        self.had_rows |= !rows.is_empty();
        if running && !self.ticking {
            self.tick(cx);
        }
        if let Some(mtm) = MainThreadMarker::new() {
            let tall = height(rows.len(), mtm);
            if tall != self.tall {
                self.tall = tall;
                cx.defer_in(window, move |_, window, _| {
                    window.resize(size(window.bounds().size.width, tall));
                });
            }
        }
        let last = rows.len().saturating_sub(1);
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .flex()
                .flex_col()
                .when(rows.is_empty(), |body| {
                    body.child(
                        div().h(px(ROW_H)).flex().items_center().child(
                            native_label("ng.finder.progress.none", "No operations in progress")
                                .font_size(tokens::small_system_font_size())
                                .color(SystemColor::SecondaryLabel)
                                .align(LabelAlign::Center),
                        ),
                    )
                })
                .children(rows.into_iter().enumerate().map(|(at, row)| {
                    div().relative().child(row).when(at < last, |row| {
                        row.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(px(1.))
                                .bg(system_color(SystemColor::Separator)),
                        )
                    })
                })),
        )
    }
}
