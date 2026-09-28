use gpui::{App, ElementId, Style, Window};
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSSwitch};

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{Layer, NativeHandler, Slot, ValueKind, hosted_element, wire};
use crate::controls::{bool_handler, fitted, sync_enabled};

pub fn native_switch(id: impl Into<ElementId>, on: bool) -> NativeSwitch {
    NativeSwitch {
        id: id.into(),
        on,
        enabled: true,
        on_change: None,
    }
}

pub struct NativeSwitch {
    id: ElementId,
    on: bool,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeSwitch {
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(bool_handler(handler));
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn layout(&self) -> Style {
        fitted(fitting_size("NSSwitch", |mtm| {
            NSSwitch::new(mtm).fittingSize()
        }))
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let switch = slot.place(Layer::Above, |mtm, target| {
            let switch = NSSwitch::new(mtm);
            wire(&switch, target, slot.key);
            switch
        });
        let wanted = if self.on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        };
        if switch.state() != wanted {
            switch.setState(wanted);
        }
        sync_enabled(&switch, self.enabled);
        slot.on_change(ValueKind::Bool, self.on_change.take());
    }
}

hosted_element!(NativeSwitch);
