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

use gpui::{Length, Style, px, relative, size};
use objc2_foundation::NSSize;

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
    NativeTextField, native_search_field, native_secure_field, native_text_field,
};

pub(crate) fn axis(reported: f64) -> Length {
    if reported > 0. {
        px(reported as f32).into()
    } else {
        relative(1.).into()
    }
}

pub(crate) fn fitted(fitting: NSSize) -> Style {
    Style {
        size: size(axis(fitting.width), axis(fitting.height)),
        ..Default::default()
    }
}
