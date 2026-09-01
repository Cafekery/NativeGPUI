//! `NSBox` as a solid fill: a divider, a hairline, a band that has to survive a material.

use std::panic::Location;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style, Window, relative,
};
use objc2::runtime::NSObjectProtocol;
use objc2_app_kit::{NSBox, NSBoxType, NSTitlePosition};

use crate::bridge::host::{Layer, NativeHost, element_key};
use crate::system::tokens::SystemColor;

/// A rule drawn by AppKit rather than by GPUI, for the same reason [`crate::native_label`] exists:
/// a [`Layer::Above`] material blurs every GPUI pixel in its rect, and a 1pt line is the first
/// thing a blur destroys.
///
/// It fills the box you give it, so the box *is* the rule — the caller writes the geometry it
/// already measured, and there is no thickness knob to disagree with it:
///
/// ```ignore
/// div().w(px(1.)).h(px(19.)).child(native_rule(("column", i)))
/// div().h(px(1.)).w_full().child(native_rule("hairline"))
/// ```
///
/// Ordering and clipping are the label's: paint it after the material and it lands on top, and it
/// truncates with an `overflow_hidden` ancestor exactly like GPUI content.
pub fn native_rule(id: impl Into<ElementId>) -> NativeRule {
    NativeRule {
        id: id.into(),
        color: SystemColor::Separator,
    }
}

pub struct NativeRule {
    id: ElementId,
    color: SystemColor,
}

impl NativeRule {
    /// A role, not a value. Defaults to [`SystemColor::Separator`].
    ///
    /// The box keeps the dynamic `NSColor` rather than a resolved value, so the rule follows an
    /// appearance change: `fillColor` read back after writing `separatorColor` still compares
    /// equal to `separatorColor`, and unequal to any other colour written the same way. A
    /// `CALayer` `backgroundColor` cannot do that — it is a `CGColor`, which is one appearance's
    /// answer frozen at the moment it was written.
    pub fn color(mut self, color: SystemColor) -> Self {
        self.color = color;
        self
    }
}

impl Element for NativeRule {
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
        // Both axes free: a fill has no opinion about its own size, and `NSBox`'s `fittingSize` is
        // its title's, which a title-less box does not have.
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
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

        let rule = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            let rule = NSBox::new(mtm);
            // `Custom` is the only box type that draws `fillColor`, so the colour knob would be
            // silently inert under any other. Filled with an unmistakable magenta and cached to a
            // bitmap offscreen, the centre pixel comes back magenta under `Custom`, fully
            // transparent under `Separator`, and the box's own near-clear white under `Primary`.
            rule.setBoxType(NSBoxType::Custom);
            rule.setBorderWidth(0.);
            rule.setTitlePosition(NSTitlePosition::NoTitle);
            rule
        });

        // `NSColor` round-trips through `fillColor`, so this diffs against the readback rather than
        // against a recorded intent — the same measurement that lets the label diff its font.
        let color = self.color.ns_color();
        if !rule.fillColor().isEqual(Some(&**color)) {
            rule.setFillColor(&color);
        }
    }
}

impl IntoElement for NativeRule {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
