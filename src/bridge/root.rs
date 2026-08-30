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
/// At rest AppKit and GPUI land on the same pixel: 0.000 pt between a GPUI-painted band and a
/// hosted view at the same layout position. Under motion the hosted view leads its own row by
/// `2 x velocity`, returning to 0 the instant motion stops.
///
/// That lead is the Metal layer's drawable queue depth, not a commit-ordering problem — it is
/// exactly 2 frames at every velocity, and dropping `CAMetalLayer.maximumDrawableCount` from its
/// default 3 to 2 makes it exactly 1. One frame is irreducible: the floor is 2, and asking for 1
/// leaves `nextDrawable` unsatisfiable. The default stands because 2 buffers cost about twice the
/// long-frame rate and 3-4 ms at p95 under load, to buy back 12 pt at an ordinary scroll speed.
/// `NG_SKEW=<pt/frame>` in `examples/gallery` re-measures it; add `NG_DRAWABLES=2` for the arm
/// that halves the lead.
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
        // Any mouse-down GPUI sees is one no hosted view claimed, because the host's container
        // returns null from `hitTest:` outside its real subviews. That is exactly when keyboard
        // focus should come back from a hosted text field.
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
/// `Transparent` is what makes `NSWindow.isOpaque` false, and that is the difference between a
/// [`crate::Layer::Below`] material showing the desktop through a hole in your GPUI scene and it
/// rendering as flat grey. A real `NSSplitViewItem` sidebar needs it too — on an opaque window the
/// system sidebar renders flat dark grey, 2.8% backdrop transmission against 26.8%. It costs
/// nothing when no material is placed: opaque GPUI drawing is bit-identical either way, 0 of
/// 81,472 pixels differing.
///
/// One consequence to know about, because it is invisible until someone notices the shadow. On a
/// **transparent** window macOS derives the drop shadow from **content alpha**, not from the window
/// frame. Paint opaquely out to the window edge near each corner and the shadow follows the
/// rounded corner exactly, matching an opaque window's (diagonal corner inset 15.56px either way).
/// Leave an unpainted band reaching a corner — a glass hole that runs to the edge, a sidebar whose
/// last row is not filled — and the shadow follows the painted content instead: a rounded window
/// with a square shadow.
///
/// `appears_transparent` becomes `NSFullSizeContentViewWindowMask`, which is what lets GPUI draw
/// under the titlebar — and means **GPUI's content view covers the traffic lights and any
/// toolbar**. A full-bleed opaque background paints straight over them and hides them completely.
/// Inset your opaque chrome by the live titlebar band, and read that band as
/// `frame.height - window.contentLayoutRect().height`, never
/// `frame.height - contentRectForFrameRect(frame).height` — under this style mask the content rect
/// *is* the frame, so the latter reads a constant 0.00 while the band is really 32pt bare, 52pt
/// with a unified toolbar and 40pt with a compact one.
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
