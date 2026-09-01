//! `NSTextField` in AppKit's label configuration: static text, no editing, no background.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::panic::Location;

use gpui::{
    App, Bounds, Element, ElementId, FontWeight, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, Style, Window, px, relative,
};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2_app_kit::{NSFont, NSLineBreakMode, NSTextAlignment, NSTextField};
use objc2_foundation::NSString;

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{Layer, NativeHost, element_key};
use crate::system::tokens::{
    SystemColor, ns_font, ns_font_weight, system_font_family, system_font_size,
};

/// Any string with an ascender and a descender. `fittingSize`'s height does not depend on the
/// string — 16.000 at 13pt for `Ag`, `Name`, a fifteen-character number and the empty string
/// alike — which is what lets the cache key below leave the text out. It does depend on newlines,
/// which is why the text this control draws is one line.
const MEASURE: &str = "Ag";

/// Static text drawn by AppKit rather than by GPUI.
///
/// Reach for it when text has to stay sharp over a [`Layer::Above`] material. Such a material
/// samples and blurs *every* GPUI pixel in its rect, its own caption included, so a band whose
/// backdrop blurs the content scrolling under it cannot label itself with GPUI text. Ordering is
/// GPUI paint order — paint the material first and the labels after:
///
/// ```ignore
/// div().relative()
///     .child(div().absolute().inset_0().child(
///         material("band", Material::Header).layer(Layer::Above).blending(Blending::WithinWindow)))
///     .child(div().w(px(168.)).child(native_label("name", "Name")))
/// ```
///
/// The label fills the box you give it and truncates at the tail, the way a table header cell
/// does: [`NativeLabel::align`] positions the text inside that box, not inside the text's own
/// fitting size. It carries no `Styled` impl, as no control here does, so the box is a wrapping
/// `div`.
///
/// One line: it reserves one line's height, and a `\n` in the text lays out a second line AppKit
/// then clips. `setUsesSingleLineMode:` does not prevent that — measured on macOS 26.5, a label
/// carrying `Name\nDate Modified` reports 32.0 points of height with the flag set and 32.0
/// without, so the flag is not the guard a reader would expect it to be.
pub fn native_label(id: impl Into<ElementId>, text: &str) -> NativeLabel {
    NativeLabel {
        id: id.into(),
        text: SharedString::from(text.to_owned()),
        font_size: None,
        weight: FontWeight::NORMAL,
        color: SystemColor::Label,
        align: LabelAlign::default(),
        tabular: false,
        truncates: true,
    }
}

/// Where the text sits in the box the caller gave the label.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LabelAlign {
    /// The leading edge for the writing direction, AppKit's own default.
    #[default]
    Natural,
    Left,
    Center,
    Right,
}

impl From<LabelAlign> for NSTextAlignment {
    fn from(align: LabelAlign) -> Self {
        match align {
            LabelAlign::Natural => NSTextAlignment::Natural,
            LabelAlign::Left => NSTextAlignment::Left,
            LabelAlign::Center => NSTextAlignment::Center,
            LabelAlign::Right => NSTextAlignment::Right,
        }
    }
}

pub struct NativeLabel {
    id: ElementId,
    text: SharedString,
    font_size: Option<Pixels>,
    weight: FontWeight,
    color: SystemColor,
    align: LabelAlign,
    tabular: bool,
    truncates: bool,
}

impl NativeLabel {
    /// Defaults to [`crate::tokens::system_font_size`].
    pub fn font_size(mut self, font_size: Pixels) -> Self {
        self.font_size = Some(font_size);
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    /// A role, not a value: AppKit re-resolves it when the appearance changes, which an `Hsla`
    /// baked at paint time could not.
    pub fn color(mut self, color: SystemColor) -> Self {
        self.color = color;
        self
    }

    pub fn align(mut self, align: LabelAlign) -> Self {
        self.align = align;
        self
    }

    /// Digits all one width, for a column of numbers that must not shift as its digits change.
    ///
    /// The AppKit twin of [`crate::tokens::system_font_tabular`], and the same face at the same
    /// weight rather than a monospaced one. Ten `1`s and ten `4`s — SF's narrowest and widest
    /// digits — measure 74.00/74.00 at 11pt, 86.00/86.00 at 13pt and 91.00/91.00 at 14pt with
    /// this on, against 56.00/76.00, 64.00/87.00 and 68.00/93.00 with it off.
    pub fn tabular(mut self, tabular: bool) -> Self {
        self.tabular = tabular;
        self
    }

    /// An ellipsis at the tail when the text does not fit, on by default. Off clips mid-glyph.
    pub fn truncates(mut self, truncates: bool) -> Self {
        self.truncates = truncates;
        self
    }

    fn point_size(&self) -> Pixels {
        self.font_size.unwrap_or_else(system_font_size)
    }

    fn font(&self) -> Retained<NSFont> {
        let size = self.point_size();
        if self.tabular {
            NSFont::monospacedDigitSystemFontOfSize_weight(
                f32::from(size) as f64,
                ns_font_weight(self.weight),
            )
        } else {
            ns_font(system_font_family(), size, self.weight)
                .expect("the system family always resolves")
        }
    }
}

impl Element for NativeLabel {
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
        let mut hasher = DefaultHasher::new();
        f32::from(self.point_size()).to_bits().hash(&mut hasher);
        self.weight.0.to_bits().hash(&mut hasher);
        self.tabular.hash(&mut hasher);
        let font = self.font();
        let fitting = fitting_size_keyed("NSTextField(label)", hasher.finish(), |mtm| {
            let probe = NSTextField::labelWithString(&NSString::from_str(MEASURE), mtm);
            probe.setFont(Some(&font));
            probe.fittingSize()
        });

        let mut style = Style::default();
        // Width is free by choice, not because AppKit declares it so: a label's own `fittingSize`
        // width is a function of its text, and `fitting_size_keyed`'s cache is never evicted, so
        // keying it on the string would leak an entry per distinct value — four header titles is
        // nothing, a date or size column is unbounded. The caller's box drives the width instead,
        // and the text truncates or aligns inside it.
        style.size.width = relative(1.).into();
        style.size.height = px(fitting.height as f32).into();
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

        let field = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            // `labelWithString:` is AppKit's own static-text recipe and needs no undoing: bezeled,
            // bordered, editable, selectable and drawsBackground all read back false and the
            // cell already does not wrap, so the line break mode below is the only thing to set.
            NSTextField::labelWithString(&NSString::from_str(""), mtm)
        });

        let want: &str = &self.text;
        if field.stringValue().to_string() != want {
            field.setStringValue(&NSString::from_str(want));
        }

        // Every write below invalidates the intrinsic size and marks the view for display, so each
        // one is diffed. `isEqual:` is safe here where a component's own recorded intent would
        // normally be needed: AppKit hands back the identical `NSFont` for a repeated
        // `systemFont(ofSize:weight:)` and the identical `NSColor` for a repeated semantic
        // colour, and both compare unequal across size, weight, the tabular descriptor and role.
        let font = self.font();
        if !field.font().is_some_and(|have| have.isEqual(Some(&**font))) {
            field.setFont(Some(&font));
        }

        let color = self.color.ns_color();
        if !field
            .textColor()
            .is_some_and(|have| have.isEqual(Some(&**color)))
        {
            field.setTextColor(Some(&color));
        }

        let align = NSTextAlignment::from(self.align);
        if field.alignment() != align {
            field.setAlignment(align);
        }

        let breaks = if self.truncates {
            NSLineBreakMode::ByTruncatingTail
        } else {
            NSLineBreakMode::ByClipping
        };
        if field.lineBreakMode() != breaks {
            field.setLineBreakMode(breaks);
        }
    }
}

impl IntoElement for NativeLabel {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
