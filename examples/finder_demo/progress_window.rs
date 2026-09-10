use std::cell::RefCell;
use std::collections::HashSet;

use gpui::{
    AnyElement, App, Bounds, Context, EntityId, Subscription, Window, WindowBounds, WindowHandle,
    WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{native_root, window_options};

use crate::app::FinderDemo;
use crate::menus::ShowProgressWindow;
use crate::ops::jobs::job_row;
use crate::palette::Palette;

const WIDTH: f32 = 420.;
const HEIGHT: f32 = 300.;

thread_local! {
    static OPEN: RefCell<Option<WindowHandle<Progress>>> = const { RefCell::new(None) };
}

/// Finder's Window ▸ Show Progress Window: one window for every operation in the app, brought
/// forward when it is already open.
pub(crate) fn show(_: &ShowProgressWindow, cx: &mut App) {
    if let Some(open) = OPEN.with_borrow(|open| *open) {
        if open
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The menu fires inside a Finder window's update, and the first draw reads every Finder window,
    // so the window opens once that update has unwound.
    cx.defer(|cx| {
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(160.), px(160.)),
                    size: size(px(WIDTH), px(HEIGHT)),
                })),
                ..window_options()
            },
            |window, cx| {
                window.set_window_title("Progress");
                cx.new(|_| Progress {
                    watched: HashSet::new(),
                    watching: Vec::new(),
                })
            },
        );
        if let Ok(opened) = opened {
            OPEN.with_borrow_mut(|open| *open = Some(opened));
        }
    });
}

/// Each Finder window keeps its own jobs and notifies as they move, so this watches every one of
/// them, taking in the windows opened since it last drew.
struct Progress {
    watched: HashSet<EntityId>,
    watching: Vec<Subscription>,
}

impl Render for Progress {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demos: Vec<_> = cx
            .windows()
            .into_iter()
            .filter_map(|window| window.downcast::<FinderDemo>()?.entity(cx).ok())
            .collect();
        for demo in &demos {
            if self.watched.insert(demo.entity_id()) {
                self.watching.push(cx.observe(demo, |_, _, cx| cx.notify()));
            }
        }
        let palette = Palette::new(false);
        let rows: Vec<AnyElement> = demos
            .iter()
            .flat_map(|demo| {
                demo.read(cx)
                    .jobs
                    .iter()
                    .map(|job| {
                        let (demo, id) = (demo.clone(), job.id);
                        job_row(job, &palette, move |cx| {
                            demo.update(cx, |this, cx| {
                                this.queue.cancel(id);
                                cx.notify();
                            })
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        // The rows host native progress bars, which only a native root takes away again.
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(px(34.))
                .px(px(12.))
                .pb(px(12.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .font_family(tokens::system_font_family())
                .text_color(palette.label)
                .when(rows.is_empty(), |body| {
                    body.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.secondary)
                            .child("No operations in progress"),
                    )
                })
                .children(rows),
        )
    }
}
