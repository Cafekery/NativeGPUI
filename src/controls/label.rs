use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::{App, ElementId, FontWeight, Pixels, SharedString, Style, Window, px, relative};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{ClassType, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSFont, NSLineBreakMode, NSResponder, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSString};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::controls::menu::nsstring_is;
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
        let mut hasher = DefaultHasher::new();
        f32::from(self.point_size()).to_bits().hash(&mut hasher);
        self.weight.0.to_bits().hash(&mut hasher);
        self.tabular.hash(&mut hasher);
        let fitting = fitting_size_keyed("NSTextField(label)", hasher.finish(), |mtm| {
            let probe = NSTextField::labelWithString(&NSString::from_str("Ag"), mtm);
            probe.setFont(Some(&self.font()));
            probe.fittingSize()
        });
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(fitting.height as f32).into();
        style
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let field = slot.place(Layer::Above, |mtm, _| Label::new(mtm));

        let want: &str = &self.text;
        if !nsstring_is(&field.stringValue(), want) {
            field.setStringValue(&NSString::from_str(want));
        }
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
    fn new(_mtm: MainThreadMarker) -> Retained<Self> {
        let empty = NSString::from_str("");
        unsafe { msg_send![Self::class(), labelWithString: &*empty] }
    }
}
