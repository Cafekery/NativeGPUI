use std::cell::Cell;
use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, EntityId, Pixels, Subscription, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{
    LabelAlign, RawMaterial, material, native_label, native_root, remember_frame, titlebar_band,
    window_options,
};
use objc2::MainThreadMarker;

use crate::app::FinderDemo;
use crate::chrome::toolbar::{system_corners, titled_height};
use crate::menus::ShowProgressWindow;
use crate::ops::jobs::{HAIRLINE_Y, LAST_ROW_H, ROW_H, WIDTH, progress_row};

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
        let rows = row_heights(&demos(cx), cx);
        let tall = height(&rows, mtm);
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
                        had_rows: !rows.is_empty(),
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

fn row_heights(demos: &[Entity<FinderDemo>], cx: &App) -> Vec<f32> {
    demos
        .iter()
        .flat_map(|demo| demo.read(cx).jobs.iter().filter(|job| job.shown()))
        .map(|job| job.height())
        .collect()
}

fn height(rows: &[f32], mtm: MainThreadMarker) -> Pixels {
    let content = rows.iter().sum::<f32>() - (ROW_H - LAST_ROW_H);
    titled_height(content.max(LAST_ROW_H), mtm)
}

struct Progress {
    watched: HashSet<EntityId>,
    watching: Vec<Subscription>,
    tall: Pixels,
    /// Checked when a job changes, not on draw: a covered window never draws.
    had_rows: bool,
    ticking: bool,
}

impl Progress {
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

    fn jobs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = demos(cx).iter().any(|demo| demo.read(cx).progress_shown());
        if self.had_rows && !shown {
            cx.defer_in(window, |_, window, _| window.remove_window());
        }
        self.had_rows |= shown;
        cx.notify();
    }
}

impl Render for Progress {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demos = demos(cx);
        for demo in &demos {
            if self.watched.insert(demo.entity_id()) {
                self.watching
                    .push(cx.observe_in(demo, window, |this, _, window, cx| {
                        this.jobs_changed(window, cx)
                    }));
            }
        }
        let mut running = false;
        let mut rows = Vec::new();
        let mut heights = Vec::new();
        for demo in &demos {
            for job in demo.read(cx).jobs.iter().filter(|job| job.shown()) {
                running |= job.running();
                heights.push(job.height());
                rows.push(progress_row(job, demo));
            }
        }
        self.had_rows |= !rows.is_empty();
        if running && !self.ticking {
            self.tick(cx);
        }
        if let Some(mtm) = MainThreadMarker::new() {
            let tall = height(&heights, mtm);
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
                .relative()
                .child(
                    material(
                        "ng.finder.progress.background",
                        RawMaterial::WindowBackground,
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    div()
                        .size_full()
                        .pt(titlebar_band(window))
                        .flex()
                        .flex_col()
                        .when(rows.is_empty(), |body| {
                            body.child(
                                div().h(px(LAST_ROW_H)).flex().items_center().child(
                                    native_label(
                                        "ng.finder.progress.none",
                                        "No operations in progress",
                                    )
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
                                        .bottom(px(ROW_H - HAIRLINE_Y - 1.))
                                        .h(px(1.))
                                        .bg(system_color(SystemColor::Separator)),
                                )
                            })
                        })),
                ),
        )
    }
}
