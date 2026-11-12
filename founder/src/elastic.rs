use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Pixels, Point, ScrollHandle, ScrollWheelEvent, TouchPhase, Window,
    div, point, prelude::*, px,
};

use crate::app::FinderDemo;

const PULL: f32 = 0.0745;
const SPRING: f32 = 12.5;
const BOUNCE: f32 = 0.31;
const APART: f32 = 0.05;

#[derive(Default)]
pub(crate) struct Elastic {
    x: Band,
    y: Band,
    held: bool,
    last: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct Bands {
    pub(crate) rows: Elastic,
    pub(crate) sideways: Elastic,
    pub(crate) icons: Elastic,
    pub(crate) strip: Elastic,
    pub(crate) browser: Elastic,
    pub(crate) columns: Vec<Elastic>,
}

impl Bands {
    pub(crate) fn tick(&mut self) -> bool {
        [
            &mut self.rows,
            &mut self.sideways,
            &mut self.icons,
            &mut self.strip,
            &mut self.browser,
        ]
        .into_iter()
        .chain(&mut self.columns)
        .fold(false, |moving, band| band.tick() | moving)
    }
}

pub(crate) fn shifted(item: AnyElement, by: Pixels) -> AnyElement {
    // The list lays each item out as a root, and a root's own relative offset is ignored.
    if by == px(0.) {
        return item;
    }
    div()
        .w_full()
        .child(div().w_full().relative().top(by).child(item))
        .into_any_element()
}

/// Must run after the scroller's own wheel listener: `wheel` reads the offset it wrote.
pub(crate) fn stretching(
    band: impl Fn(&mut FinderDemo) -> (&mut Elastic, ScrollHandle) + 'static,
    cx: &mut Context<FinderDemo>,
) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static {
    let view = cx.entity().downgrade();
    move |event, _, cx| {
        view.update(cx, |this, cx| {
            let (elastic, handle) = band(this);
            elastic.wheel(event, &handle);
            cx.notify();
        })
        .ok();
    }
}

#[derive(Default)]
struct Band {
    pulled: f32,
    stretch: f32,
    spring: Option<Spring>,
    rest: f32,
    speed: f32,
}

#[derive(Clone, Copy)]
struct Spring {
    at: Instant,
    from: f32,
    speed: f32,
    /// A release waits for the momentum behind it, if any, to give its speed.
    waiting: bool,
}

#[derive(Clone, Copy)]
struct Turn {
    held: bool,
    released: bool,
    moved: bool,
    apart: f32,
    now: Instant,
}

impl Spring {
    fn at(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.at).as_secs_f32();
        (self.from + BOUNCE * self.speed * t) * (-SPRING * t).exp()
    }

    fn done(&self, now: Instant) -> bool {
        let t = now.saturating_duration_since(self.at).as_secs_f32();
        t * SPRING > 1. && self.at(now).abs() < 0.5
    }
}

impl Elastic {
    fn wheel(&mut self, event: &ScrollWheelEvent, handle: &ScrollHandle) {
        if !event.delta.precise() {
            return;
        }
        let now = Instant::now();
        let apart = self
            .last
            .map_or(f32::MAX, |last| (now - last).as_secs_f32());
        self.last = Some(now);
        let released = match event.touch_phase {
            TouchPhase::Started => {
                self.held = true;
                false
            }
            TouchPhase::Ended | TouchPhase::Cancelled => std::mem::take(&mut self.held),
            TouchPhase::Moved => false,
        };
        let turn = Turn {
            held: self.held,
            released,
            moved: event.delta.pixel_delta(px(0.)) != Point::default(),
            apart,
            now,
        };
        let (offset, max, size) = (handle.offset(), handle.max_offset(), handle.bounds().size);
        let x = if max.x > px(0.) {
            px(self.x.follow(offset.x, max.x, size.width, turn))
        } else {
            offset.x.clamp(-max.x, px(0.))
        };
        let y = px(self.y.follow(offset.y, max.y, size.height, turn));
        handle.set_offset(point(x, y));
    }

    pub(crate) fn shown(&self) -> Point<Pixels> {
        let now = Instant::now();
        point(px(self.x.shown(now)), px(self.y.shown(now)))
    }

    fn tick(&mut self) -> bool {
        let now = Instant::now();
        for band in [&mut self.x, &mut self.y] {
            band.spring = band.spring.filter(|spring| !spring.done(now));
        }
        self.x.spring.is_some() || self.y.spring.is_some()
    }
}

impl Band {
    fn shown(&self, now: Instant) -> f32 {
        self.spring
            .map_or(self.stretch, |spring| spring.at(now))
            .round()
    }

    fn follow(&mut self, offset: Pixels, max: Pixels, extent: Pixels, turn: Turn) -> f32 {
        let (offset, max, extent) = (f32::from(offset), f32::from(max), f32::from(extent).max(1.));
        let edge = if turn.held {
            if let Some(spring) = self.spring.filter(|_| turn.moved) {
                let caught = spring.at(turn.now);
                self.pulled = caught.signum() * extent / PULL * caught.abs()
                    / (extent - caught.abs()).max(1.);
                self.spring = None;
            }
            let wanted = offset + self.pulled;
            let edge = wanted.clamp(-max, 0.);
            self.pulled = wanted - edge;
            let x = self.pulled.abs();
            self.stretch = self.pulled.signum() * (1. - 1. / (PULL * x / extent + 1.)) * extent;
            edge
        } else {
            let edge = offset.clamp(-max, 0.);
            let speed = self.speed_to(offset, turn);
            match &mut self.spring {
                Some(spring) if spring.waiting && !turn.released => {
                    if speed * spring.from > 0. {
                        *spring = Spring {
                            at: turn.now,
                            from: spring.at(turn.now),
                            speed,
                            waiting: false,
                        };
                    }
                    spring.waiting = false;
                }
                Some(_) => {}
                None if turn.released && self.pulled != 0. => {
                    self.spring = Some(Spring {
                        at: turn.now,
                        from: self.stretch,
                        speed: 0.,
                        waiting: true,
                    });
                }
                // Momentum that runs into the edge carries past it before springing back.
                None if (offset - edge) * self.speed > 0. => {
                    self.spring = Some(Spring {
                        at: turn.now,
                        from: 0.,
                        speed: (speed + self.speed) / 2.,
                        waiting: false,
                    });
                }
                None => {}
            }
            if turn.released {
                self.pulled = 0.;
                self.stretch = 0.;
            }
            edge
        };
        if turn.moved {
            self.speed = self.speed_to(edge, turn);
        }
        self.rest = edge;
        edge
    }

    fn speed_to(&self, offset: f32, turn: Turn) -> f32 {
        if turn.apart < APART {
            (offset - self.rest) / turn.apart.max(0.001)
        } else {
            0.
        }
    }
}
