use gpui::{
    AnyElement, AnyView, App, Bounds, DispatchPhase, EntityId, IntoElement, MouseDownEvent, Pixels,
    StyleRefinement, Styled, TitlebarOptions, Window, WindowBackgroundAppearance, WindowOptions,
};

use crate::bridge::appkit::focus_gpui;
use crate::bridge::host::NativeHost;

macro_rules! wrapper_element {
    ($ty:ty $(, before_layout: $before:path)?) => {
        impl gpui::Element for $ty {
            type RequestLayoutState = ();
            type PrepaintState = ();

            fn id(&self) -> Option<gpui::ElementId> {
                None
            }

            fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
                None
            }

            fn request_layout(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) -> (gpui::LayoutId, ()) {
                $($before(window, cx);)?
                (self.child.request_layout(window, cx), ())
            }

            fn prepaint(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                _: gpui::Bounds<gpui::Pixels>,
                _: &mut (),
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) {
                self.child.prepaint(window, cx);
            }

            fn paint(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                bounds: gpui::Bounds<gpui::Pixels>,
                _: &mut (),
                _: &mut (),
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) {
                <$ty>::paint(self, bounds, window, cx);
            }
        }

        impl gpui::IntoElement for $ty {
            type Element = Self;

            fn into_element(self) -> Self {
                self
            }
        }
    };
}
pub(crate) use wrapper_element;

pub fn native_root(child: impl IntoElement) -> NativeRoot {
    NativeRoot {
        child: child.into_any_element(),
    }
}

/// Replays `view` from gpui's cache, its hosted AppKit views left placed; `fresh` draws it anew.
pub fn native_cached(
    view: impl Into<AnyView>,
    style: StyleRefinement,
    fresh: bool,
) -> NativeCached {
    let view = view.into();
    NativeCached {
        owner: view.entity_id(),
        fresh,
        child: if fresh {
            view.into_any_element()
        } else {
            view.cached(style).into_any_element()
        },
    }
}

pub struct NativeCached {
    owner: EntityId,
    fresh: bool,
    child: AnyElement,
}

impl NativeCached {
    fn paint(&mut self, _: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let host = NativeHost::get(window, cx);
        let from = host.placed();
        self.child.paint(window, cx);
        host.keep(self.owner, from, self.fresh);
    }
}

wrapper_element!(NativeCached);

/// `child` drawn at `opacity`, the AppKit views it hosts faded with it.
pub fn native_opacity(opacity: f32, child: impl Styled + IntoElement) -> NativeOpacity {
    NativeOpacity {
        opacity,
        child: child.opacity(opacity).into_any_element(),
    }
}

pub struct NativeOpacity {
    opacity: f32,
    child: AnyElement,
}

impl NativeOpacity {
    fn paint(&mut self, _: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let host = NativeHost::get(window, cx);
        let outer = host.alpha();
        host.set_alpha(outer * self.opacity);
        self.child.paint(window, cx);
        host.set_alpha(outer);
    }
}

wrapper_element!(NativeOpacity);

pub struct NativeRoot {
    child: AnyElement,
}

impl NativeRoot {
    fn paint(&mut self, _: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        window.on_mouse_event(|_: &MouseDownEvent, phase, window, _| {
            if phase == DispatchPhase::Capture {
                focus_gpui(window);
            }
        });
        self.child.paint(window, cx);
        NativeHost::get(window, cx).commit();
    }
}

wrapper_element!(NativeRoot, before_layout: NativeHost::get);

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
