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
/// AppKit and GPUI land on the same pixel at rest and under motion alike. They did not always:
/// GPUI's presented Metal surface trails the `CATransaction` that carried the same frame's layout
/// by `CAMetalLayer.maximumDrawableCount - 1` frames, so writing a hosted view its *current*
/// layout put it ahead of its own GPUI row by that many frames of velocity — measured as exactly
/// 2.000 frames at 4, 8 and 16 points per frame. [`crate::NativeHost::place`] writes the presented
/// frame's layout instead, which reads 0.000 at the same velocities and 0.000 at rest after an
/// abrupt stop. `NG_SKEW` in `examples/gallery` re-measures both arms; `NG_LAG=0` is the
/// uncompensated one.
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
/// One consequence is the opposite of what it looks like: the drop shadow does **not** come from
/// what you paint. An unpainted band reaching a corner — a glass hole running to the edge, a
/// sidebar whose last row is not filled — leaves the shadow byte-identical. The silhouette tracks
/// the window frame and its corner radius; see [`crate::set_corner_radius`] for the three arms.
///
/// `appears_transparent` becomes `NSFullSizeContentViewWindowMask`, which lets GPUI draw under the
/// titlebar and so means **GPUI's content view covers the traffic lights and any toolbar**: a
/// full-bleed opaque background hides them completely. Inset opaque chrome by the live titlebar
/// band, read as `frame.height - window.contentLayoutRect().height` and never
/// `frame.height - contentRectForFrameRect(frame).height` — under this style mask the content rect
/// *is* the frame, so the latter reads a constant 0.00 while the band is really 32pt or more.
///
/// Neither arm of `WindowBackgroundAppearance` is where a window's background colour comes from.
/// `Opaque` writes sRGB black and turns off the renderer's transparency, and under `Transparent`
/// gpui writes an all-but-clear `alpha = 0.0001`; setting `NSWindow.backgroundColor` yourself is
/// then inert, because a window-spanning backdrop or the Metal layer covers it. Writing it from
/// that clear value to an opaque `windowBackgroundColor` — readback in the same call confirming
/// the write landed — left the window capture byte-identical. Paint the background in the view
/// that owns the surface; see [`crate::tokens::WINDOW_BACKGROUND`].
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
