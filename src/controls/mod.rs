mod button;
mod image;
mod label;
pub(crate) mod menu;
mod progress;
mod rule;
mod scroller;
mod slider;
mod small;
mod switch;
mod text_field;

use std::ffi::CStr;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use gpui::{App, Length, Pixels, Style, Window, px, relative, size};
use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::{NSControl, NSControlSize, NSFont, NSImage, NSView};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{ControlSize, Layer, NativeHandler, NativeValue, Slot, wire};

pub use button::{
    ButtonBezel, CheckState, NativeButton, NativeCheckbox, NativeRadio, NativeSegmented,
    SegmentStyle, TrackingMode, native_button, native_checkbox, native_radio, native_segmented,
};
pub use image::{NativeImage, native_image, native_raster, native_symbol};
pub use label::{LabelAlign, NativeLabel, native_label};
pub use menu::{
    MenuEntry, MenuOrigin, MenuView, NativeContextMenu, NativePopup, native_context_menu,
    native_popup,
};
pub use progress::{
    LevelStyle, NativeLevel, NativeProgress, NativeSpinner, native_level, native_progress,
    native_spinner,
};
pub use rule::{NativeRule, native_rule};
pub use scroller::{NativeScrollBars, native_scroll_bars};
pub use slider::{NativeSlider, native_slider};
pub use small::{
    ColorWellStyle, DateElements, DatePickerStyle, NativeColorWell, NativeDatePicker,
    NativePathControl, NativeStepper, PathStyle, native_color_well, native_date_picker,
    native_path_control, native_stepper,
};
pub use switch::{NativeSwitch, native_switch};
pub use text_field::{
    NativeTextField, native_combo_box, native_search_field, native_secure_field, native_text_field,
};

fn axis(reported: f64) -> Length {
    if reported > 0. {
        px(reported as f32).into()
    } else {
        relative(1.).into()
    }
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn fitting_size_of(
    class: &'static str,
    inputs: impl Hash,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    fitting_size_keyed(class, hash_of(inputs), measure)
}

fn measured(
    class: &'static str,
    inputs: impl Hash,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> Style {
    let fitting = fitting_size_of(class, inputs, measure);
    Style {
        size: size(axis(fitting.width), axis(fitting.height)),
        ..Default::default()
    }
}

fn with_width(mut style: Style, width: Option<Length>) -> Style {
    if let Some(width) = width {
        style.size.width = width;
    }
    style
}

/// A fractional origin blurs a hosted control's strokes and glyphs.
fn snap_origin(slot: &mut Slot, scale: f32) {
    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
    slot.bounds.origin = slot.bounds.origin.map(snap);
}

fn place_control<V>(slot: &Slot, new: impl FnOnce(MainThreadMarker) -> Retained<V>) -> Retained<V>
where
    V: Message + AsRef<NSView> + AsRef<NSControl> + 'static,
{
    slot.place(Layer::Above, |mtm, target| {
        let control = new(mtm);
        let wired: &NSControl = (*control).as_ref();
        wire(wired, target, slot.key);
        control
    })
}

fn sync_enabled(control: &NSControl, enabled: bool) {
    if control.isEnabled() != enabled {
        control.setEnabled(enabled);
    }
}

fn sync_control_size(control: &NSControl, size: ControlSize) {
    let size = NSControlSize::from(size);
    if control.controlSize() != size {
        control.setControlSize(size);
    }
}

fn sync_font_size(control: &NSControl, size: Option<f64>) {
    let Some(size) = size else {
        return;
    };
    if control.font().map(|font| font.pointSize()) != Some(size) {
        control.setFont(Some(&NSFont::systemFontOfSize(size)));
    }
}

fn nsstring_is(text: &NSString, want: &str) -> bool {
    let ptr = text.UTF8String();
    if ptr.is_null() {
        return want.is_empty();
    }
    unsafe { CStr::from_ptr(ptr) }.to_bytes() == want.as_bytes()
}

fn symbol_image(name: &str, description: Option<&str>) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        description.map(NSString::from_str).as_deref(),
    )
}

fn bool_handler(handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> NativeHandler {
    Rc::new(move |value, window, cx| {
        if let NativeValue::Bool(on) = value {
            handler(&on, window, cx);
        }
    })
}

fn number_handler(handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> NativeHandler {
    Rc::new(move |value, window, cx| {
        if let NativeValue::Number(number) = value {
            handler(&number, window, cx);
        }
    })
}
