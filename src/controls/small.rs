use std::any::Any;
use std::rc::Rc;

use gpui::{App, ElementId, Hsla, SharedString, Style, Window, px, relative};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBackgroundStyle, NSColor, NSColorWell, NSColorWellStyle, NSDatePicker,
    NSDatePickerElementFlags, NSDatePickerStyle, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSLineBreakMode, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSPathControl, NSPathControlItem, NSPathStyle, NSStepper,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDate, NSMutableAttributedString, NSRange, NSString, NSURL,
};

use crate::bridge::appkit::{fitting_size, fitting_size_keyed};
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHandler, NativeRead, NativeValue, Slot, ValueKind, hosted_element,
    wire,
};
use crate::controls::fitted;
use crate::system::color::{from_nscolor, to_nscolor};

type Handler<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// A reader's `None` means the control had nothing to report.
fn forward_some<T: 'static>(user: Option<Handler<T>>) -> Option<NativeDispatch> {
    user.map(|user| -> NativeDispatch {
        Rc::new(move |value: Box<dyn Any>, window, cx| {
            if let Ok(Some(value)) = value.downcast::<Option<T>>().map(|value| *value) {
                user(&value, window, cx);
            }
        })
    })
}

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
    /// Inclusive; unbounded by default.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn increment(mut self, increment: f64) -> Self {
        self.increment = increment;
        self
    }

    pub fn wraps(mut self, wraps: bool) -> Self {
        self.wraps = wraps;
        self
    }

    pub fn autorepeat(mut self, autorepeat: bool) -> Self {
        self.autorepeat = autorepeat;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Number(value) = value {
                handler(&value, window, cx);
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        fitted(fitting_size("NSStepper", |mtm| {
            NSStepper::new(mtm).fittingSize()
        }))
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let stepper = slot.place(Layer::Above, |mtm, target| {
            let stepper = NSStepper::new(mtm);
            wire(&stepper, target, slot.key);
            stepper
        });
        // Bounds before value: AppKit clamps `doubleValue` to the range currently set.
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
        slot.on_change(ValueKind::Number, self.on_change.take());
    }
}

hosted_element!(NativeStepper);

/// `Swatch` opens the system colour panel; the other two open a popover picker.
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
    on_change: Option<Handler<Hsla>>,
}

impl NativeColorWell {
    pub fn style(mut self, style: ColorWellStyle) -> Self {
        self.style = style;
        self
    }

    pub fn supports_alpha(mut self, supports_alpha: bool) -> Self {
        self.supports_alpha = supports_alpha;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Fires continuously in the picker; skipped for a colour with no RGB representation.
    pub fn on_change(mut self, handler: impl Fn(&Hsla, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    fn layout(&self) -> Style {
        let style = self.style;
        fitted(fitting_size_keyed("NSColorWell", style as u64, |mtm| {
            NSColorWell::colorWellWithStyle(style.appkit(), mtm).fittingSize()
        }))
    }

    fn paint(&mut self, slot: Slot, window: &mut Window, _: &mut App) {
        let style = self.style.appkit();
        let well = slot.place(Layer::Above, |mtm, target| {
            let well = NSColorWell::colorWellWithStyle(style, mtm);
            wire(&well, target, slot.key);
            well
        });
        if well.colorWellStyle() != style {
            well.setColorWellStyle(style);
        }
        if well.supportsAlpha() != self.supports_alpha {
            well.setSupportsAlpha(self.supports_alpha);
        }
        // Diffed against the value last pushed, never the live well: a stale write would land in
        // a picker the user is dragging in.
        let target = self.color;
        window.with_element_state::<Hsla, _>(slot.id, |pushed, _| {
            if pushed != Some(target) && from_nscolor(&well.color()) != Some(target) {
                well.setColor(&to_nscolor(target));
            }
            ((), target)
        });
        if well.isEnabled() != self.enabled {
            well.setEnabled(self.enabled);
        }
        let read: NativeRead = Rc::new(|sender: &AnyObject| {
            let color: Retained<NSColor> = unsafe { msg_send![sender, color] };
            Box::new(from_nscolor(&color)) as Box<dyn Any>
        });
        slot.on_event(read, forward_some(self.on_change.take()));
    }
}

hosted_element!(NativeColorWell);

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum DatePickerStyle {
    /// Editable fields with a stepper.
    #[default]
    Stepper,
    /// The calendar grid and analog clock.
    Graphical,
    /// Editable fields, no stepper.
    Text,
}

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

/// `seconds` is seconds since the Unix epoch, UTC.
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
    on_change: Option<Handler<f64>>,
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

    fn layout(&self) -> Style {
        let (appkit_style, elements) = (self.appkit_style(), self.elements);
        let key = (self.style as u64) << 3 | elements as u64;
        fitted(fitting_size_keyed("NSDatePicker", key, move |mtm| {
            let picker = NSDatePicker::new(mtm);
            picker.setDatePickerStyle(appkit_style);
            picker.setDatePickerElements(elements.appkit());
            picker.fittingSize()
        }))
    }

    fn paint(&mut self, slot: Slot, window: &mut Window, _: &mut App) {
        let picker = slot.place(Layer::Above, |mtm, target| {
            let picker = NSDatePicker::new(mtm);
            wire(&picker, target, slot.key);
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
        let target = self.seconds;
        window.with_element_state::<f64, _>(slot.id, |pushed, _| {
            if pushed != Some(target) && picker.dateValue().timeIntervalSince1970() != target {
                picker.setDateValue(&NSDate::dateWithTimeIntervalSince1970(target));
            }
            ((), target)
        });
        if picker.isEnabled() != self.enabled {
            picker.setEnabled(self.enabled);
        }
        let read: NativeRead = Rc::new(|sender: &AnyObject| {
            let date: Retained<NSDate> = unsafe { msg_send![sender, dateValue] };
            Box::new(Some(date.timeIntervalSince1970())) as Box<dyn Any>
        });
        slot.on_event(read, forward_some(self.on_change.take()));
    }
}

hosted_element!(NativeDatePicker);

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum PathStyle {
    /// The Finder-style breadcrumb.
    #[default]
    Standard,
    /// A popup showing the leaf, with the ancestors in its menu.
    PopUp,
}

/// Fills the width it is given and pins only its height.
pub fn native_path_control(id: impl Into<ElementId>, path: impl Into<String>) -> NativePathControl {
    NativePathControl {
        id: id.into(),
        path: path.into(),
        style: PathStyle::default(),
        enabled: true,
        titles: None,
        bold: Vec::new(),
        emphasized: false,
        on_click: None,
    }
}

pub struct NativePathControl {
    id: ElementId,
    path: String,
    style: PathStyle,
    enabled: bool,
    titles: Option<Vec<SharedString>>,
    bold: Vec<usize>,
    emphasized: bool,
    on_click: Option<Handler<String>>,
}

impl NativePathControl {
    /// These components in place of the path's own, without icons; a click reports the whole path.
    pub fn titles(mut self, titles: Vec<SharedString>) -> Self {
        self.titles = Some(titles);
        self
    }

    /// Characters of the last title, by index, drawn bold as a search match is.
    pub fn bold(mut self, chars: Vec<usize>) -> Self {
        self.bold = chars;
        self
    }

    /// For a selected row: titles and chevrons in the selected text colour.
    pub fn emphasized(mut self, emphasized: bool) -> Self {
        self.emphasized = emphasized;
        self
    }

    pub fn style(mut self, style: PathStyle) -> Self {
        self.style = style;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Not called for a click that landed on no component.
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

    fn layout(&self) -> Style {
        let appkit_style = self.appkit_style();
        let fitting = fitting_size_keyed("NSPathControl", self.style as u64, move |mtm| {
            let control = NSPathControl::new(mtm);
            control.setPathStyle(appkit_style);
            // With no components the control fits 0x0 and never appears.
            control.setURL(Some(&NSURL::fileURLWithPath(&NSString::from_str("/"))));
            control.fittingSize()
        });
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(fitting.height as f32).into();
        style
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let control = slot.place(Layer::Above, |mtm, target| {
            let control = NSPathControl::new(mtm);
            wire(&control, target, slot.key);
            control
        });
        let appkit_style = self.appkit_style();
        if control.pathStyle() != appkit_style {
            control.setPathStyle(appkit_style);
        }
        match &self.titles {
            Some(titles) => set_titles(&control, titles, &self.bold),
            None => {
                let current = control
                    .URL()
                    .and_then(|url| url.path())
                    .map(|path| path.to_string());
                if current.as_deref() != Some(self.path.as_str()) {
                    control.setURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
                        &self.path,
                    ))));
                }
            }
        }
        let background = match self.emphasized {
            true => NSBackgroundStyle::Emphasized,
            false => NSBackgroundStyle::Normal,
        };
        if let Some(cell) = control.cell() {
            if cell.backgroundStyle() != background {
                cell.setBackgroundStyle(background);
            }
        }
        if control.isEnabled() != self.enabled {
            control.setEnabled(self.enabled);
        }
        // Titled items carry no URL, so a click on one reports the path the titles stand for.
        let whole = self.titles.is_some().then(|| self.path.clone());
        // `clickedPathItem` is nil once the action returns, so it is read inside it.
        let read: NativeRead = Rc::new(move |sender: &AnyObject| {
            let item: Option<Retained<NSPathControlItem>> =
                unsafe { msg_send![sender, clickedPathItem] };
            let path = item
                .and_then(|item| item.URL())
                .and_then(|url| url.path())
                .map(|path| path.to_string())
                .or_else(|| whole.clone());
            Box::new(path) as Box<dyn Any>
        });
        slot.on_event(read, forward_some(self.on_click.take()));
    }
}

hosted_element!(NativePathControl);

/// Rebuilds the items only when a title or its bold characters changed.
fn set_titles(control: &NSPathControl, titles: &[SharedString], bold: &[usize]) {
    let size = control
        .font()
        .map_or_else(NSFont::systemFontSize, |font| font.pointSize());
    let (plain, heavy) = (
        NSFont::systemFontOfSize(size),
        NSFont::boldSystemFontOfSize(size),
    );
    let last = titles.len().saturating_sub(1);
    let wanted: Vec<Retained<NSAttributedString>> = titles
        .iter()
        .enumerate()
        .map(|(at, title)| {
            let marks = if at == last { bold } else { &[] };
            attributed(title, &plain, &heavy, marks)
        })
        .collect();
    let items = control.pathItems();
    let same = items.count() == wanted.len()
        && items
            .iter()
            .zip(&wanted)
            .all(|(item, title)| item.attributedTitle().isEqualToAttributedString(title));
    if same {
        return;
    }
    let items: Vec<Retained<NSPathControlItem>> = wanted
        .iter()
        .map(|title| {
            let item = NSPathControlItem::new();
            item.setAttributedTitle(title);
            item
        })
        .collect();
    control.setPathItems(&NSArray::from_retained_slice(&items));
}

fn attributed(
    text: &str,
    plain: &NSFont,
    heavy: &NSFont,
    bold: &[usize],
) -> Retained<NSAttributedString> {
    let string = NSMutableAttributedString::from_nsstring(&NSString::from_str(text));
    let whole = NSRange::new(0, string.length());
    // Without a colour the cell draws titles secondary; an emphasized cell still turns it white.
    let color = NSColor::labelColor();
    // Clipped rather than truncated: a squeezed title fades into its chevron, as Finder's do.
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(NSLineBreakMode::ByClipping);
    unsafe {
        string.addAttribute_value_range(NSFontAttributeName, plain, whole);
        string.addAttribute_value_range(NSForegroundColorAttributeName, &color, whole);
        string.addAttribute_value_range(NSParagraphStyleAttributeName, &style, whole);
    }
    // `bold` counts chars; attributed ranges count UTF-16 units.
    let mut unit = 0;
    for (at, ch) in text.chars().enumerate() {
        let len = ch.len_utf16();
        if bold.contains(&at) {
            let range = NSRange::new(unit, len);
            unsafe { string.addAttribute_value_range(NSFontAttributeName, heavy, range) };
        }
        unit += len;
    }
    Retained::into_super(string)
}
