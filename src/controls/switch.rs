//! `NSSwitch`.

use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style, Window, px, size,
};
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSSwitch};

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{
    Layer, NativeHandler, NativeHost, NativeValue, ValueKind, element_key, wire,
};

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
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Bool(on) = value {
                handler(&on, window, cx);
            }
        }));
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

impl Element for NativeSwitch {
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
        let fitting = fitting_size("NSSwitch", |mtm| NSSwitch::new(mtm).fittingSize());
        let mut style = Style::default();
        style.size = size(
            px(fitting.width as f32).into(),
            px(fitting.height as f32).into(),
        );
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
        let host = NativeHost::get(window, cx);

        let switch = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let switch = NSSwitch::new(mtm);
            wire(&switch, target, key);
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
        if switch.isEnabled() != self.enabled {
            switch.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::Bool, self.on_change.take());
    }
}

impl IntoElement for NativeSwitch {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
