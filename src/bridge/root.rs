//! The wrapper that owns the frame boundary for native views.

use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, MouseDownEvent, Pixels, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowOptions,
};

use crate::bridge::appkit::focus_gpui;
use crate::bridge::host::NativeHost;

/// Wrap your window's root element in this, once.
///
/// It creates the window's [`NativeHost`] before anything paints, and commits every hosted view's
/// geometry in one pass once your whole tree has painted.
///
/// At rest AppKit and GPUI land on the same pixel. Under motion a hosted view leads its own GPUI
/// row by `2 x velocity`, returning to 0 the instant motion stops: that lead is the Metal layer's
/// drawable queue depth, not a commit-ordering problem — dropping
/// `CAMetalLayer.maximumDrawableCount` from its default 3 to 2 halves it to one frame, and 2 is the
/// floor, since a count of 1 leaves `nextDrawable` unsatisfiable. The default stands because the
/// extra buffer costs more in long frames than the lead is worth. `NG_SKEW` in `examples/gallery`
/// re-measures it.
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
        // Any mouse-down GPUI sees is one no hosted view claimed — the containers return null from
        // `hitTest:` outside their real subviews — which is exactly when focus should come back.
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

/// Window options a native-looking app wants.
///
/// `Transparent` makes `NSWindow.isOpaque` false, which is the difference between a
/// [`crate::Layer::Below`] material showing the desktop through a hole in your GPUI scene and it
/// rendering as flat grey. A real `NSSplitViewItem` sidebar needs it too, or the system sidebar
/// renders flat dark grey. It costs nothing when no material is placed: opaque GPUI drawing is
/// bit-identical either way.
///
/// It has one consequence that stays invisible until someone notices the shadow. On a
/// **transparent** window macOS derives the drop shadow from **content alpha**, not from the window
/// frame. Paint opaquely out to the window edge near each corner and the shadow follows the rounded
/// corner exactly, matching an opaque window's. Leave an unpainted band reaching a corner — a glass
/// hole that runs to the edge, a sidebar whose last row is not filled — and the shadow follows the
/// painted content instead: a rounded window with a square shadow.
///
/// `appears_transparent` becomes `NSFullSizeContentViewWindowMask`, which lets GPUI draw under the
/// titlebar and so means **GPUI's content view covers the traffic lights and any toolbar**: a
/// full-bleed opaque background hides them completely. Inset opaque chrome by the live titlebar
/// band, read as `frame.height - window.contentLayoutRect().height` and never
/// `frame.height - contentRectForFrameRect(frame).height` — under this style mask the content rect
/// *is* the frame, so the latter reads a constant 0.00 while the band is really 32pt or more.
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
