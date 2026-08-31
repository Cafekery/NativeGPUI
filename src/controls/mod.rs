//! Hosted AppKit controls. Put these in your GPUI layout.
//!
//! Each `paint` does the same three things: derive the pool key from GPUI's element identity,
//! [`crate::NativeHost::place`] the view, then push only the properties that changed.

mod button;
pub(crate) mod menu;
mod progress;
mod slider;
mod small;
mod switch;
mod text_field;

pub use button::{
    ButtonBezel, CheckState, NativeButton, NativeCheckbox, NativeRadio, NativeSegmented,
    TrackingMode, native_button, native_checkbox, native_radio, native_segmented,
};
pub use menu::{
    MenuEntry, MenuOrigin, MenuView, NativeContextMenu, NativePopup, native_context_menu,
    native_popup,
};
pub use progress::{
    LevelStyle, NativeLevel, NativeProgress, NativeSpinner, native_level, native_progress,
    native_spinner,
};
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
