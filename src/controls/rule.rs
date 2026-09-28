use gpui::{App, ElementId, Hsla, Size, Style, Window};
use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBox, NSBoxType, NSResponder, NSTitlePosition, NSView};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect};

use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::system::color::to_nscolor;
use crate::system::tokens::SystemColor;

pub fn native_rule(id: impl Into<ElementId>) -> NativeRule {
    NativeRule {
        id: id.into(),
        color: SystemColor::Separator,
        tint: None,
        corner_radius: 0.,
    }
}

pub struct NativeRule {
    id: ElementId,
    color: SystemColor,
    tint: Option<Hsla>,
    corner_radius: f64,
}

impl NativeRule {
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = f64::from(radius);
        self
    }

    pub fn color(mut self, color: SystemColor) -> Self {
        self.color = color;
        self
    }

    pub fn tint(mut self, tint: impl Into<Hsla>) -> Self {
        self.tint = Some(tint.into());
        self
    }

    fn layout(&self) -> Style {
        Style {
            size: Size::full(),
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let rule = slot.place(Layer::Above, |mtm, _| Rule::new(mtm));
        let color = match self.tint {
            Some(tint) => to_nscolor(tint),
            None => self.color.ns_color(),
        };
        if rule.fillColor() != color {
            rule.setFillColor(&color);
        }
        if rule.cornerRadius() != self.corner_radius {
            rule.setCornerRadius(self.corner_radius);
        }
    }
}

hosted_element!(NativeRule);

define_class!(
    #[unsafe(super(NSBox, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGRule"]
    struct Rule;

    impl Rule {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl Rule {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this: Retained<Self> =
            unsafe { msg_send![Self::alloc(mtm), initWithFrame: NSRect::ZERO] };
        this.setBoxType(NSBoxType::Custom);
        this.setBorderWidth(0.);
        this.setTitlePosition(NSTitlePosition::NoTitle);
        this
    }
}
