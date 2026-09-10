use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseDownEvent, Pixels, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowOptions,
};

use crate::bridge::appkit::focus_gpui;
use crate::bridge::host::NativeHost;

pub fn native_root(child: impl IntoElement) -> NativeRoot {
    NativeRoot {
        child: child.into_any_element(),
    }
}

pub struct NativeRoot {
    child: AnyElement,
}

impl Element for NativeRoot {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
        NativeHost::get(window, cx);
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.on_mouse_event(|_: &MouseDownEvent, phase, window, _| {
            if phase == DispatchPhase::Capture {
                focus_gpui(window);
            }
        });
        self.child.paint(window, cx);
        NativeHost::get(window, cx).commit();
    }
}

impl IntoElement for NativeRoot {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

pub fn window_options() -> WindowOptions {
    WindowOptions {
        window_background: WindowBackgroundAppearance::Transparent,
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}
