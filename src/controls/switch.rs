use gpui::{App, ElementId, Style, Window};
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSBackingStoreType, NSControlStateValueOff, NSControlStateValueOn, NSSwitch, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::NSRect;

use crate::bridge::host::{
    ControlSize, Layer, NativeHandler, Slot, ValueKind, hosted_element, wire,
};
use crate::controls::{bool_handler, measured, sync_control_size, sync_enabled};

pub fn native_switch(id: impl Into<ElementId>, on: bool) -> NativeSwitch {
    NativeSwitch {
        id: id.into(),
        on,
        enabled: true,
        control_size: ControlSize::default(),
        on_change: None,
    }
}

pub struct NativeSwitch {
    id: ElementId,
    on: bool,
    enabled: bool,
    control_size: ControlSize,
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

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    fn layout(&self) -> Style {
        measured("NSSwitch", self.control_size, |mtm| {
            // Out of a window, a switch reports its regular size at every control size.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::ZERO,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    true,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            let switch = NSSwitch::new(mtm);
            switch.setControlSize(self.control_size.into());
            if let Some(content) = window.contentView() {
                content.addSubview(&switch);
            }
            switch.fittingSize()
        })
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
        sync_control_size(&switch, self.control_size);
        if switch.state() != wanted {
            switch.setState(wanted);
        }
        sync_enabled(&switch, self.enabled);
        slot.on_change(ValueKind::Bool, self.on_change.take());
    }
}

hosted_element!(NativeSwitch);
