use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, ElementId, Style, Window};
use objc2_app_kit::NSSlider;

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{
    Layer, NativeHandler, NativeValue, Slot, ValueKind, hosted_element, wire,
};
use crate::controls::fitted;

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

/// The value last written. `doubleValue` does not round-trip under snapping or clamping.
#[derive(Clone, Default)]
struct Held(Rc<Cell<Option<f64>>>);

impl NativeSlider {
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn ticks(mut self, count: usize) -> Self {
        self.ticks = count;
        self
    }

    pub fn snap_to_ticks(mut self, snap: bool) -> Self {
        self.snap_to_ticks = snap;
        self
    }

    pub fn vertical(mut self, vertical: bool) -> Self {
        self.vertical = vertical;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Number(value) = value {
                handler(&value, window, cx);
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        let (vertical, ticks) = (self.vertical, self.ticks);
        let class = match (vertical, ticks > 0) {
            (false, false) => "NSSlider",
            (false, true) => "NSSlider/ticks",
            (true, false) => "NSSlider/vertical",
            (true, true) => "NSSlider/vertical+ticks",
        };
        fitted(fitting_size(class, move |mtm| {
            let slider = NSSlider::new(mtm);
            if vertical {
                slider.setVertical(true);
            }
            if ticks > 0 {
                slider.setNumberOfTickMarks(ticks as isize);
            }
            slider.fittingSize()
        }))
    }

    fn paint(&mut self, slot: Slot, window: &mut Window, _: &mut App) {
        let held = window.with_element_state::<Held, _>(slot.id, |state, _| {
            let state = state.unwrap_or_default();
            (state.clone(), state)
        });
        let fresh = Cell::new(false);
        let slider = slot.place(Layer::Above, |mtm, target| {
            let slider = NSSlider::new(mtm);
            wire(&slider, target, slot.key);
            slider.setContinuous(true);
            fresh.set(true);
            slider
        });
        if fresh.get() {
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

        let handler = self.on_change.take().map(|handler| -> NativeHandler {
            let held = held.0.clone();
            Rc::new(move |value, window, cx| {
                if let NativeValue::Number(number) = value {
                    held.set(Some(number));
                }
                handler(value, window, cx);
            })
        });
        slot.on_change(ValueKind::Number, handler);
    }
}

hosted_element!(NativeSlider);
