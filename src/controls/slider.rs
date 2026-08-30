//! `NSSlider`, including the macOS 26 glass knob.

use std::cell::Cell;
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Length, Pixels, Style, Window, px, relative, size,
};
use objc2_app_kit::NSSlider;
use objc2_foundation::NSSize;

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{
    Layer, NativeHandler, NativeHost, NativeValue, ValueKind, element_key, wire,
};

/// Values outside the range are clamped, by AppKit and here.
pub fn native_slider(id: impl Into<ElementId>, value: f64) -> NativeSlider {
    NativeSlider {
        id: id.into(),
        value,
        min: 0.,
        max: 1.,
        ticks: 0,
        snap_to_ticks: false,
        vertical: false,
        enabled: true,
        on_change: None,
    }
}

pub struct NativeSlider {
    id: ElementId,
    value: f64,
    min: f64,
    max: f64,
    ticks: usize,
    snap_to_ticks: bool,
    vertical: bool,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeSlider {
    /// Defaults to `0.0..=1.0`.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    /// `count` includes both ends, so 5 ticks on `0.0..=1.0` sit at 0, 0.25, 0.5, 0.75 and 1.
    pub fn ticks(mut self, count: usize) -> Self {
        self.ticks = count;
        self
    }
    /// Restrict the value to the tick marks. Does nothing without [`NativeSlider::ticks`].
    pub fn snap_to_ticks(mut self, snap: bool) -> Self {
        self.snap_to_ticks = snap;
        self
    }
    /// Swaps the axes: a vertical slider pins its width and fills the height.
    pub fn vertical(mut self, vertical: bool) -> Self {
        self.vertical = vertical;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Fires continuously while the knob is dragged, one frame after the position it reports.
    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Number(value) = value {
                handler(&value, window, cx);
            }
        }));
        self
    }

    fn fitting(&self) -> NSSize {
        let vertical = self.vertical;
        let ticks = self.ticks;
        let class = match (vertical, ticks > 0) {
            (false, false) => "NSSlider",
            (false, true) => "NSSlider/ticks",
            (true, false) => "NSSlider/vertical",
            (true, true) => "NSSlider/vertical+ticks",
        };
        fitting_size(class, move |mtm| {
            let slider = NSSlider::new(mtm);
            if vertical {
                slider.setVertical(true);
            }
            if ticks > 0 {
                slider.setNumberOfTickMarks(ticks as isize);
            }
            slider.fittingSize()
        })
    }
}

/// A zero axis from `fittingSize` means AppKit declares that axis free, so layout owns it.
fn axis(reported: f64) -> Length {
    if reported > 0. {
        px(reported as f32).into()
    } else {
        relative(1.).into()
    }
}

/// The value the control currently holds. `doubleValue` does not round-trip once snapping or
/// clamping is in play, so it is diffed against intent rather than read back.
#[derive(Clone, Default)]
struct Held(Rc<Cell<Option<f64>>>);

impl Element for NativeSlider {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let fitting = self.fitting();
        let mut style = Style::default();
        style.size = size(axis(fitting.width), axis(fitting.height));
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let mask = window.content_mask().bounds;

        let held = match id {
            Some(id) => window.with_element_state::<Held, _>(id, |state, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            }),
            None => Held::default(),
        };

        let fresh = Cell::new(false);
        let host = NativeHost::get(window, cx);
        let slider = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let slider = NSSlider::new(mtm);
            wire(&slider, target, key);
            if !slider.isContinuous() {
                slider.setContinuous(true);
            }
            fresh.set(true);
            slider
        });
        if fresh.get() {
            // A brand new control holds 0, whatever a surviving `Held` remembers.
            held.0.set(None);
        }

        if slider.isVertical() != self.vertical {
            slider.setVertical(self.vertical);
        }
        // Range before value: the value is clamped into the range as it is written.
        if slider.minValue() != self.min {
            slider.setMinValue(self.min);
            held.0.set(None);
        }
        if slider.maxValue() != self.max {
            slider.setMaxValue(self.max);
            held.0.set(None);
        }
        if slider.numberOfTickMarks() != self.ticks as isize {
            slider.setNumberOfTickMarks(self.ticks as isize);
        }
        let snap = self.snap_to_ticks && self.ticks > 0;
        if slider.allowsTickMarkValuesOnly() != snap {
            slider.setAllowsTickMarkValuesOnly(snap);
        }
        let wanted = self.value.clamp(self.min, self.max);
        if held.0.get() != Some(wanted) {
            slider.setDoubleValue(wanted);
            held.0.set(Some(wanted));
        }
        if slider.isEnabled() != self.enabled {
            slider.setEnabled(self.enabled);
        }

        let handler = self.on_change.take().map(|handler| {
            let held = held.0.clone();
            Rc::new(
                move |value: NativeValue, window: &mut Window, cx: &mut App| {
                    if let NativeValue::Number(number) = value {
                        held.set(Some(number));
                    }
                    handler(value, window, cx);
                },
            ) as NativeHandler
        });
        host.on_change(key, ValueKind::Number, handler);
    }
}

impl IntoElement for NativeSlider {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
