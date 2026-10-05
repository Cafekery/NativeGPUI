use std::cell::RefCell;
use std::collections::HashSet;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, EntityId, Pixels, Subscription, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{LabelAlign, native_label, native_root, titlebar_band, window_options};
use objc2::MainThreadMarker;
use objc2_app_kit::NSWindowStyleMask;

use crate::app::FinderDemo;
use crate::chrome::toolbar::{titlebar_corners, titled_height};
use crate::frame::ns_window;
use crate::menus::ShowProgressWindow;
use crate::ops::jobs::{ROW_H, progress_row};

const WIDTH: f32 = 399.;

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<Progress>>> = const { RefCell::new(None) };
}

pub(crate) fn shown() -> bool {
    OPEN.with_borrow(|open| open.is_some())
}

pub(crate) fn toggle(_: &ShowProgressWindow, cx: &mut App) {
    if let Some(open) = OPEN.with_borrow(|open| *open) {
        open.update(cx, |_, window, _| window.remove_window()).ok();
        return;
    }
    // Deferred: the first draw reads every Finder window, and the menu fires inside one's update.
    cx.defer(|cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let rows = rows_shown(&demos(cx), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(160.), px(160.)),
                    size: size(px(WIDTH), height(rows, mtm)),
                })),
                ..window_options()
            },
            |window, cx| {
                window.set_window_title("");
                titlebar_corners(window);
                if let Some(ns) = ns_window(window) {
                    ns.setStyleMask(ns.styleMask() & !NSWindowStyleMask::Closable);
                }
                cx.new(|cx| {
                    cx.on_release(|_, _| OPEN.with_borrow_mut(|open| *open = None))
                        .detach();
                    Progress {
                        watched: HashSet::new(),
                        watching: Vec::new(),
                        rows,
                    }
                })
            },
        );
        if let Ok(opened) = opened {
            OPEN.with_borrow_mut(|open| *open = Some(opened));
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
        .map(|demo| demo.read(cx).jobs.len())
        .sum::<usize>()
        .max(1)
}

fn height(rows: usize, mtm: MainThreadMarker) -> Pixels {
    titled_height(rows as f32 * ROW_H, mtm)
}

struct Progress {
    watched: HashSet<EntityId>,
    watching: Vec<Subscription>,
    rows: usize,
}

impl Render for Progress {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demos = demos(cx);
        for demo in &demos {
            if self.watched.insert(demo.entity_id()) {
                self.watching.push(cx.observe(demo, |_, _, cx| cx.notify()));
            }
        }
        let rows = rows_shown(&demos, cx);
        if let Some(mtm) = MainThreadMarker::new().filter(|_| rows != self.rows) {
            self.rows = rows;
            let tall = height(rows, mtm);
            cx.defer_in(window, move |_, window, _| {
                window.resize(size(window.bounds().size.width, tall));
            });
        }
        let jobs: Vec<AnyElement> = demos
            .iter()
            .flat_map(|demo| {
                demo.read(cx).jobs.iter().map(|job| {
                    let (demo, id) = (demo.clone(), job.id);
                    progress_row(job, move |cx| {
                        demo.update(cx, |this, cx| {
                            this.queue.cancel(id);
                            cx.notify();
                        })
                    })
                })
            })
            .collect();
        // The rows host native progress bars, which only a native root removes again.
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(titlebar_band(window))
                .flex()
                .flex_col()
                .when(jobs.is_empty(), |body| {
                    body.child(
                        div().h(px(ROW_H)).flex().items_center().child(
                            native_label("ng.finder.progress.none", "No operations in progress")
                                .font_size(tokens::small_system_font_size())
                                .color(SystemColor::SecondaryLabel)
                                .align(LabelAlign::Center),
                        ),
                    )
                })
                .children(jobs),
        )
    }
}
