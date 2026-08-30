//! The small controls: `NSStepper`, `NSColorWell`, `NSDatePicker`, `NSPathControl`. Nothing here
//! draws — the calendar grid, the clock, the colour picker and the folder icons are all macOS's.

use std::any::Any;
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Style, Window, px, size,
};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSColor, NSColorWell, NSColorWellStyle, NSDatePicker, NSDatePickerElementFlags,
    NSDatePickerStyle, NSPathControl, NSPathControlItem, NSPathStyle, NSStepper,
};
use objc2_foundation::{NSDate, NSString, NSURL};

use crate::bridge::appkit::{fitting_size, fitting_size_keyed};
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue, ValueKind,
    element_key, wire,
};
use crate::system::color::{from_nscolor, to_nscolor};

/// A reader's `None` means the control had nothing to report, so the handler is not called.
fn forward_some<T: 'static>(
    user: Option<Rc<dyn Fn(&T, &mut Window, &mut App)>>,
) -> Option<NativeDispatch> {
    user.map(|user| {
        Rc::new(
            move |value: Box<dyn Any>, window: &mut Window, cx: &mut App| {
                if let Ok(Some(value)) = value.downcast::<Option<T>>().map(|value| *value) {
                    user(&value, window, cx);
                }
            },
        ) as NativeDispatch
    })
}

/// A real macOS stepper. `value` is the number you own; wire [`NativeStepper::on_change`]. To pair
/// one with a text field, render both from a single `f64` rather than wiring them to each other.
pub fn native_stepper(id: impl Into<ElementId>, value: f64) -> NativeStepper {
    NativeStepper {
        id: id.into(),
        value,
        min: f64::MIN,
        max: f64::MAX,
        increment: 1.,
        wraps: false,
        autorepeat: true,
        enabled: true,
        on_change: None,
    }
}

pub struct NativeStepper {
    id: ElementId,
    value: f64,
    min: f64,
    max: f64,
    increment: f64,
    wraps: bool,
    autorepeat: bool,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeStepper {
    /// Inclusive bounds AppKit clamps to. Unbounded by default.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn increment(mut self, increment: f64) -> Self {
        self.increment = increment;
        self
    }
    /// Whether stepping past a bound wraps to the other one. Only meaningful with a [`Self::range`].
    pub fn wraps(mut self, wraps: bool) -> Self {
        self.wraps = wraps;
        self
    }
    /// Whether press-and-hold repeats. Default `true`, as in AppKit.
    pub fn autorepeat(mut self, autorepeat: bool) -> Self {
        self.autorepeat = autorepeat;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// The value is already clamped and wrapped by AppKit; do not re-clamp it.
    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Number(value) = value {
                handler(&value, window, cx);
            }
        }));
        self
    }
}

impl Element for NativeStepper {
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
        let fitting = fitting_size("NSStepper", |mtm| NSStepper::new(mtm).fittingSize());
        let mut style = Style::default();
        style.size = size(
            px(fitting.width as f32).into(),
            px(fitting.height as f32).into(),
        );
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

        let stepper = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let stepper = NSStepper::new(mtm);
            wire(&stepper, target, key);
            stepper
        });

        // Bounds before value: AppKit clamps `doubleValue` to the range currently set, so pushing
        // the value against last frame's range would quietly truncate it.
        if stepper.minValue() != self.min {
            stepper.setMinValue(self.min);
        }
        if stepper.maxValue() != self.max {
            stepper.setMaxValue(self.max);
        }
        if stepper.increment() != self.increment {
            stepper.setIncrement(self.increment);
        }
        if stepper.valueWraps() != self.wraps {
            stepper.setValueWraps(self.wraps);
        }
        if stepper.autorepeat() != self.autorepeat {
            stepper.setAutorepeat(self.autorepeat);
        }
        if stepper.doubleValue() != self.value {
            stepper.setDoubleValue(self.value);
        }
        if stepper.isEnabled() != self.enabled {
            stepper.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::Number, self.on_change.take());
    }
}

impl IntoElement for NativeStepper {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// How a colour well presents itself. [`Self::Swatch`] opens the system colour panel; the other
/// two open a popover picker.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ColorWellStyle {
    #[default]
    Swatch,
    Minimal,
    Expanded,
}

impl ColorWellStyle {
    fn appkit(self) -> NSColorWellStyle {
        match self {
            Self::Swatch => NSColorWellStyle::Default,
            Self::Minimal => NSColorWellStyle::Minimal,
            Self::Expanded => NSColorWellStyle::Expanded,
        }
    }
}

/// A real macOS colour well. `color` is the colour you own; wire [`NativeColorWell::on_change`].
pub fn native_color_well(id: impl Into<ElementId>, color: Hsla) -> NativeColorWell {
    NativeColorWell {
        id: id.into(),
        color,
        style: ColorWellStyle::default(),
        supports_alpha: true,
        enabled: true,
        on_change: None,
    }
}

pub struct NativeColorWell {
    id: ElementId,
    color: Hsla,
    style: ColorWellStyle,
    supports_alpha: bool,
    enabled: bool,
    on_change: Option<Rc<dyn Fn(&Hsla, &mut Window, &mut App)>>,
}

impl NativeColorWell {
    pub fn style(mut self, style: ColorWellStyle) -> Self {
        self.style = style;
        self
    }
    /// Whether the picker offers an opacity control. Default `true`.
    pub fn supports_alpha(mut self, supports_alpha: bool) -> Self {
        self.supports_alpha = supports_alpha;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Fires continuously while the user moves through the picker, and is skipped entirely for a
    /// colour with no RGB representation rather than reporting it as black.
    pub fn on_change(mut self, handler: impl Fn(&Hsla, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl Element for NativeColorWell {
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
        let style = self.style;
        let fitting = fitting_size_keyed("NSColorWell", style as u64, |mtm| {
            NSColorWell::colorWellWithStyle(style.appkit(), mtm).fittingSize()
        });
        let mut layout = Style::default();
        layout.size = size(
            px(fitting.width as f32).into(),
            px(fitting.height as f32).into(),
        );
        (window.request_layout(layout, [], cx), ())
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
        let style = self.style;

        let well = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let well = NSColorWell::colorWellWithStyle(style.appkit(), mtm);
            wire(&well, target, key);
            well
        });

        if well.colorWellStyle() != style.appkit() {
            well.setColorWellStyle(style.appkit());
        }
        if well.supportsAlpha() != self.supports_alpha {
            well.setSupportsAlpha(self.supports_alpha);
        }
        // Diffed against the value last pushed, never the live well: only the app changing `color`
        // is a reason to write, and a stale write would land in a picker the user is dragging in.
        let target = self.color;
        window.with_element_state::<Hsla, _>(
            id.expect("native components always return an ElementId"),
            |pushed, _| {
                if pushed != Some(target) && from_nscolor(&well.color()) != Some(target) {
                    well.setColor(&to_nscolor(target));
                }
                ((), target)
            },
        );
        if well.isEnabled() != self.enabled {
            well.setEnabled(self.enabled);
        }

        // Read at fire time inside `ngAction:`: a repaint landing between the action and the
        // deferred handler would push the app's stale colour back and be read as the new value.
        let read: NativeRead = Rc::new(|sender: &AnyObject| {
            let color: Retained<NSColor> = unsafe { msg_send![sender, color] };
            Box::new(from_nscolor(&color)) as Box<dyn Any>
        });
        host.on_event(key, read, forward_some(self.on_change.take()));
    }
}

impl IntoElement for NativeColorWell {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// How a date picker presents itself.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum DatePickerStyle {
    /// Editable text fields with a stepper.
    #[default]
    Stepper,
    /// The calendar grid and analog clock.
    Graphical,
    /// Editable text fields, no stepper.
    Text,
}

/// Which fields a date picker shows. A closed set rather than free flags: the underlying
/// `NSDatePickerElementFlags` are overlapping masks, so `YearMonthDay` contains `YearMonth` and
/// `HourMinuteSecond` contains `HourMinute`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum DateElements {
    Date,
    YearMonth,
    Time,
    TimeSeconds,
    #[default]
    DateTime,
    DateTimeSeconds,
}

impl DateElements {
    fn appkit(self) -> NSDatePickerElementFlags {
        let ymd = NSDatePickerElementFlags::YearMonthDay;
        let hm = NSDatePickerElementFlags::HourMinute;
        let hms = NSDatePickerElementFlags::HourMinuteSecond;
        match self {
            Self::Date => ymd,
            Self::YearMonth => NSDatePickerElementFlags::YearMonth,
            Self::Time => hm,
            Self::TimeSeconds => hms,
            Self::DateTime => ymd | hm,
            Self::DateTimeSeconds => ymd | hms,
        }
    }
}

/// A real macOS date picker. `seconds` is the value you own; wire
/// [`NativeDatePicker::on_change`].
///
/// Seconds since the Unix epoch, UTC — `NSDate.timeIntervalSince1970` round-trips exactly, with
/// no calendar or time zone in the middle. Format it with `NSDateFormatter` for display.
pub fn native_date_picker(id: impl Into<ElementId>, seconds: f64) -> NativeDatePicker {
    NativeDatePicker {
        id: id.into(),
        seconds,
        style: DatePickerStyle::default(),
        elements: DateElements::default(),
        enabled: true,
        on_change: None,
    }
}

pub struct NativeDatePicker {
    id: ElementId,
    seconds: f64,
    style: DatePickerStyle,
    elements: DateElements,
    enabled: bool,
    on_change: Option<Rc<dyn Fn(&f64, &mut Window, &mut App)>>,
}

impl NativeDatePicker {
    pub fn style(mut self, style: DatePickerStyle) -> Self {
        self.style = style;
        self
    }
    pub fn elements(mut self, elements: DateElements) -> Self {
        self.elements = elements;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// The value is seconds since the Unix epoch, UTC.
    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    fn appkit_style(&self) -> NSDatePickerStyle {
        match self.style {
            DatePickerStyle::Stepper => NSDatePickerStyle::TextFieldAndStepper,
            DatePickerStyle::Graphical => NSDatePickerStyle::ClockAndCalendar,
            DatePickerStyle::Text => NSDatePickerStyle::TextField,
        }
    }
}

impl Element for NativeDatePicker {
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
        let appkit_style = self.appkit_style();
        let elements = self.elements;
        // Size is a function of style and elements both: graphical date-only is a calendar,
        // graphical time-only a clock.
        let key = (self.style as u64) << 3 | elements as u64;
        let fitting = fitting_size_keyed("NSDatePicker", key, move |mtm| {
            let picker = NSDatePicker::new(mtm);
            picker.setDatePickerStyle(appkit_style);
            picker.setDatePickerElements(elements.appkit());
            picker.fittingSize()
        });
        let mut layout = Style::default();
        layout.size = size(
            px(fitting.width as f32).into(),
            px(fitting.height as f32).into(),
        );
        (window.request_layout(layout, [], cx), ())
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

        let picker = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let picker = NSDatePicker::new(mtm);
            wire(&picker, target, key);
            picker
        });

        let appkit_style = self.appkit_style();
        if picker.datePickerStyle() != appkit_style {
            picker.setDatePickerStyle(appkit_style);
        }
        let elements = self.elements.appkit();
        if picker.datePickerElements() != elements {
            picker.setDatePickerElements(elements);
        }
        // Diffed against the last pushed value for the reason given in `NativeColorWell::paint`:
        // against the live picker, a stale date would be written back mid-edit.
        let target = self.seconds;
        window.with_element_state::<f64, _>(
            id.expect("native components always return an ElementId"),
            |pushed, _| {
                if pushed != Some(target) && picker.dateValue().timeIntervalSince1970() != target {
                    picker.setDateValue(&NSDate::dateWithTimeIntervalSince1970(target));
                }
                ((), target)
            },
        );
        if picker.isEnabled() != self.enabled {
            picker.setEnabled(self.enabled);
        }

        // Not `ValueKind::Number`: `NSControl`'s value accessors mean nothing on a date picker.
        let read: NativeRead = Rc::new(|sender: &AnyObject| {
            let date: Retained<NSDate> = unsafe { msg_send![sender, dateValue] };
            Box::new(Some(date.timeIntervalSince1970())) as Box<dyn Any>
        });
        host.on_event(key, read, forward_some(self.on_change.take()));
    }
}

impl IntoElement for NativeDatePicker {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// How a path control presents itself.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum PathStyle {
    /// The Finder-style breadcrumb: one icon and name per path component.
    #[default]
    Standard,
    /// A single popup button showing the leaf, with the ancestors in its menu.
    PopUp,
}

/// A real macOS path control — the Finder-style breadcrumb. It fills the width it is given and
/// pins only its own height; AppKit truncates when the path does not fit.
pub fn native_path_control(id: impl Into<ElementId>, path: impl Into<String>) -> NativePathControl {
    NativePathControl {
        id: id.into(),
        path: path.into(),
        style: PathStyle::default(),
        enabled: true,
        on_click: None,
    }
}

pub struct NativePathControl {
    id: ElementId,
    path: String,
    style: PathStyle,
    enabled: bool,
    on_click: Option<Rc<dyn Fn(&String, &mut Window, &mut App)>>,
}

impl NativePathControl {
    pub fn style(mut self, style: PathStyle) -> Self {
        self.style = style;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Not called for a click that landed on no component, so the handler never receives an empty
    /// path standing in for "nothing was clicked".
    pub fn on_click(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    fn appkit_style(&self) -> NSPathStyle {
        match self.style {
            PathStyle::Standard => NSPathStyle::Standard,
            PathStyle::PopUp => NSPathStyle::PopUp,
        }
    }
}

impl Element for NativePathControl {
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
        let appkit_style = self.appkit_style();
        let fitting = fitting_size_keyed("NSPathControl", self.style as u64, move |mtm| {
            let control = NSPathControl::new(mtm);
            control.setPathStyle(appkit_style);
            // A URL first: with no components the control fits 0x0, which the host reads as
            // clipped away and it silently never appears. Every path gives the same height.
            control.setURL(Some(&NSURL::fileURLWithPath(&NSString::from_str("/"))));
            control.fittingSize()
        });
        // Height only: the reported width is a function of the path, so pinning it would size
        // every breadcrumb to whatever path was measured first.
        let mut layout = Style::default();
        layout.size.height = px(fitting.height as f32).into();
        (window.request_layout(layout, [], cx), ())
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

        let control = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let control = NSPathControl::new(mtm);
            wire(&control, target, key);
            control
        });

        let appkit_style = self.appkit_style();
        if control.pathStyle() != appkit_style {
            control.setPathStyle(appkit_style);
        }
        // Compare the string, not the `NSURL`: `NSURL` equality normalises in ways the path
        // string does not.
        let current = control
            .URL()
            .and_then(|url| url.path())
            .map(|path| path.to_string());
        if current.as_deref() != Some(self.path.as_str()) {
            control.setURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
                &self.path,
            ))));
        }
        if control.isEnabled() != self.enabled {
            control.setEnabled(self.enabled);
        }

        // AppKit clears `clickedPathItem` as soon as the action returns, so this reader runs
        // inside `ngAction:`; nil means the click landed on no component.
        let read: NativeRead = Rc::new(|sender: &AnyObject| {
            let item: Option<Retained<NSPathControlItem>> =
                unsafe { msg_send![sender, clickedPathItem] };
            let path = item
                .and_then(|item| item.URL())
                .and_then(|url| url.path())
                .map(|path| path.to_string());
            Box::new(path) as Box<dyn Any>
        });
        host.on_event(key, read, forward_some(self.on_click.take()));
    }
}

impl IntoElement for NativePathControl {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
