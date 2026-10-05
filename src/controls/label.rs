use gpui::{App, ElementId, FontWeight, Pixels, SharedString, Style, Window, px, relative, size};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSFont, NSLineBreakMode, NSResponder, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSString};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::controls::{hash_of, nsstring_is};
use crate::system::tokens::{
    SystemColor, ns_font, ns_font_weight, system_font_family, system_font_size,
};

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
        wraps: false,
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LabelAlign {
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
    wraps: bool,
}

impl NativeLabel {
    pub fn font_size(mut self, font_size: Pixels) -> Self {
        self.font_size = Some(font_size);
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = weight;
        self
    }

    pub fn color(mut self, color: SystemColor) -> Self {
        self.color = color;
        self
    }

    pub fn align(mut self, align: LabelAlign) -> Self {
        self.align = align;
        self
    }

    pub fn tabular(mut self, tabular: bool) -> Self {
        self.tabular = tabular;
        self
    }

    pub fn truncates(mut self, truncates: bool) -> Self {
        self.truncates = truncates;
        self
    }

    /// Wraps by word and takes its parent's height; the caller sizes it to its lines.
    pub fn wraps(mut self, wraps: bool) -> Self {
        self.wraps = wraps;
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

    fn layout(&self) -> Style {
        let key = hash_of((
            f32::from(self.point_size()).to_bits(),
            self.weight.0.to_bits(),
            self.tabular,
        ));
        let fitting = fitting_size_keyed("NSTextField(label)", key, |mtm| {
            let probe = NSTextField::labelWithString(&NSString::from_str("Ag"), mtm);
            probe.setFont(Some(&self.font()));
            probe.fittingSize()
        });
        let height = match self.wraps {
            true => relative(1.).into(),
            false => px(fitting.height as f32).into(),
        };
        Style {
            size: size(relative(1.).into(), height),
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let field = slot.place(Layer::Above, |mtm, _| Label::new(mtm));
        if !nsstring_is(&field.stringValue(), &self.text) {
            field.setStringValue(&NSString::from_str(&self.text));
        }
        let font = self.font();
        if field.font().as_ref() != Some(&font) {
            field.setFont(Some(&font));
        }
        let color = self.color.ns_color();
        if field.textColor().as_ref() != Some(&color) {
            field.setTextColor(Some(&color));
        }
        let align = NSTextAlignment::from(self.align);
        if field.alignment() != align {
            field.setAlignment(align);
        }
        if let Some(cell) = field.cell() {
            if cell.wraps() != self.wraps {
                cell.setWraps(self.wraps);
            }
        }
        let breaks = match (self.wraps, self.truncates) {
            (true, _) => NSLineBreakMode::ByWordWrapping,
            (false, true) => NSLineBreakMode::ByTruncatingTail,
            (false, false) => NSLineBreakMode::ByClipping,
        };
        if field.lineBreakMode() != breaks {
            field.setLineBreakMode(breaks);
        }
    }
}

hosted_element!(NativeLabel);

define_class!(
    #[unsafe(super(NSTextField, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGLabel"]
    struct Label;

    impl Label {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl Label {
    fn new(_: MainThreadMarker) -> Retained<Self> {
        let empty = NSString::from_str("");
        unsafe { msg_send![Self::class(), labelWithString: &*empty] }
    }
}
