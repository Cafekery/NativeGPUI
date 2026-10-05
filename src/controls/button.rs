use std::ptr;
use std::rc::Rc;

use gpui::{App, ElementId, FontWeight, Length, Pixels, Style, Window, px, size};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSButtonCell, NSButtonType, NSCellImagePosition, NSCellStyleMask,
    NSControlStateValue, NSControlStateValueMixed, NSControlStateValueOff, NSControlStateValueOn,
    NSFont, NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl,
};
use objc2_foundation::{MainThreadMarker, NSString};

use crate::bridge::host::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeRead, NativeValue, Slot, ValueKind,
    hosted_element, wire,
};
use crate::controls::{
    bool_handler, measured, nsstring_is, snap_origin, symbol_image, sync_control_size,
    sync_enabled, with_width,
};
use crate::system::tokens::{SystemColor, ns_font_weight, system_font_size_for};

type Titled =
    unsafe fn(&NSString, Option<&AnyObject>, Option<Sel>, MainThreadMarker) -> Retained<NSButton>;

fn titled(
    mtm: MainThreadMarker,
    make: Titled,
    title: &str,
    size: ControlSize,
) -> Retained<NSButton> {
    let button = unsafe { make(&NSString::from_str(title), None, None, mtm) };
    button.setControlSize(size.into());
    button
}

fn place_titled(slot: &Slot, make: Titled, title: &str, size: ControlSize) -> Retained<NSButton> {
    let button = slot.place(Layer::Above, |mtm, target| {
        let button = titled(mtm, make, title, size);
        wire(&button, target, slot.key);
        button
    });
    sync_control_size(&button, size);
    button
}

/// Auto Layout's width, which `fittingSize` rounds up to a whole point.
fn intrinsic_layout(class: &'static str, make: Titled, title: &str, size: ControlSize) -> Style {
    measured(class, (title, size), |mtm| {
        titled(mtm, make, title, size).intrinsicContentSize()
    })
}

fn sync_title(button: &NSButton, wanted: &str) {
    if !nsstring_is(&button.title(), wanted) {
        button.setTitle(&NSString::from_str(wanted));
    }
}

fn sync_symbol(button: &NSButton, symbol: Option<&str>, label: &str, only: bool) {
    let image = symbol.and_then(|name| symbol_image(name, Some(label)));
    if button.image() != image {
        button.setImage(image.as_deref());
    }
    let position = match symbol {
        None => NSCellImagePosition::NoImage,
        Some(_) if only => NSCellImagePosition::ImageOnly,
        Some(_) => NSCellImagePosition::ImageTrailing,
    };
    if button.imagePosition() != position {
        button.setImagePosition(position);
    }
    sync_title(button, if symbol.is_some() && only { "" } else { label });
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CheckState {
    Off,
    On,
    Mixed,
}

impl CheckState {
    fn from_native(raw: i64) -> Self {
        match raw {
            -1 => Self::Mixed,
            0 => Self::Off,
            _ => Self::On,
        }
    }

    fn to_native(self) -> NSControlStateValue {
        match self {
            Self::Off => NSControlStateValueOff,
            Self::On => NSControlStateValueOn,
            Self::Mixed => NSControlStateValueMixed,
        }
    }
}

impl From<bool> for CheckState {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

pub fn native_checkbox(id: impl Into<ElementId>, checked: bool) -> NativeCheckbox {
    NativeCheckbox {
        id: id.into(),
        state: checked.into(),
        allows_mixed: false,
        title: String::new(),
        control_size: ControlSize::default(),
        enabled: true,
        on_change: None,
    }
}

pub struct NativeCheckbox {
    id: ElementId,
    state: CheckState,
    allows_mixed: bool,
    title: String,
    control_size: ControlSize,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeCheckbox {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Supersedes the `checked` bool; `Mixed` implies [`Self::allows_mixed`].
    pub fn state(mut self, state: CheckState) -> Self {
        self.state = state;
        self.allows_mixed |= state == CheckState::Mixed;
        self
    }

    /// Also decides AppKit's click cycle, so keep it fixed for the control's lifetime.
    pub fn allows_mixed(mut self, allows_mixed: bool) -> Self {
        self.allows_mixed = allows_mixed;
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(
        mut self,
        handler: impl Fn(&CheckState, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Int(raw) = value {
                handler(&CheckState::from_native(raw), window, cx);
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        intrinsic_layout(
            "NSButton.checkbox",
            NSButton::checkboxWithTitle_target_action,
            &self.title,
            self.control_size,
        )
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let button = place_titled(
            &slot,
            NSButton::checkboxWithTitle_target_action,
            &self.title,
            self.control_size,
        );
        // Before the state: a mixed state on a two-state box is refused.
        if button.allowsMixedState() != self.allows_mixed {
            button.setAllowsMixedState(self.allows_mixed);
        }
        let wanted = self.state.to_native();
        if button.state() != wanted {
            button.setState(wanted);
        }
        sync_title(&button, &self.title);
        sync_enabled(&button, self.enabled);
        slot.on_change(ValueKind::Int, self.on_change.take());
    }
}

hosted_element!(NativeCheckbox);

/// AppKit groups radios by superview and each hosted view has its own: hold the selection.
pub fn native_radio(id: impl Into<ElementId>, selected: bool) -> NativeRadio {
    NativeRadio {
        id: id.into(),
        selected,
        title: String::new(),
        control_size: ControlSize::default(),
        enabled: true,
        on_change: None,
    }
}

pub struct NativeRadio {
    id: ElementId,
    selected: bool,
    title: String,
    control_size: ControlSize,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeRadio {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(bool_handler(handler));
        self
    }

    fn layout(&self) -> Style {
        intrinsic_layout(
            "NSButton.radio",
            NSButton::radioButtonWithTitle_target_action,
            &self.title,
            self.control_size,
        )
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let button = place_titled(
            &slot,
            NSButton::radioButtonWithTitle_target_action,
            &self.title,
            self.control_size,
        );
        let wanted = CheckState::from(self.selected).to_native();
        if button.state() != wanted {
            button.setState(wanted);
        }
        sync_title(&button, &self.title);
        sync_enabled(&button, self.enabled);
        slot.on_change(ValueKind::Bool, self.on_change.take());
    }
}

hosted_element!(NativeRadio);

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum ButtonBezel {
    #[default]
    Automatic,
    Push,
    FlexiblePush,
    SmallSquare,
    Toolbar,
    AccessoryBarAction,
    AccessoryBar,
    Glass,
    Circular,
    HelpButton,
    Badge,
    Disclosure,
    PushDisclosure,
}

impl ButtonBezel {
    pub(crate) fn to_native(self) -> NSBezelStyle {
        match self {
            Self::Automatic => NSBezelStyle::Automatic,
            Self::Push => NSBezelStyle::Push,
            Self::FlexiblePush => NSBezelStyle::FlexiblePush,
            Self::SmallSquare => NSBezelStyle::SmallSquare,
            Self::Toolbar => NSBezelStyle::Toolbar,
            Self::AccessoryBarAction => NSBezelStyle::AccessoryBarAction,
            Self::AccessoryBar => NSBezelStyle::AccessoryBar,
            Self::Glass => NSBezelStyle::Glass,
            Self::Circular => NSBezelStyle::Circular,
            Self::HelpButton => NSBezelStyle::HelpButton,
            Self::Badge => NSBezelStyle::Badge,
            Self::Disclosure => NSBezelStyle::Disclosure,
            Self::PushDisclosure => NSBezelStyle::PushDisclosure,
        }
    }
}

/// Asked of the window's default button cell: `keyEquivalent` set to `"\r"` reads back empty.
fn is_default_button(button: &NSButton) -> bool {
    let default = button
        .window()
        .and_then(|window| window.defaultButtonCell());
    match (button.cell(), default) {
        (Some(cell), Some(default)) => ptr::eq(
            Retained::as_ptr(&cell).cast::<AnyObject>(),
            Retained::as_ptr(&default).cast(),
        ),
        _ => false,
    }
}

/// AppKit flips a push-on-push-off button before its action; the caller's next paint sets it.
fn unflip() -> NativeRead {
    thread_local! {
        static READ: NativeRead = Rc::new(|sender| {
            if let Some(button) = sender.downcast_ref::<NSButton>() {
                button.setState(if button.state() == NSControlStateValueOff {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }
            Box::new(NativeValue::None)
        });
    }
    READ.with(Rc::clone)
}

pub fn native_button(id: impl Into<ElementId>, title: impl Into<String>) -> NativeButton {
    NativeButton {
        id: id.into(),
        title: title.into(),
        symbol: None,
        symbol_only: true,
        bezel: ButtonBezel::Automatic,
        control_size: ControlSize::default(),
        font_size: None,
        weight: None,
        on: None,
        border_on_hover: false,
        square: None,
        width: None,
        default_button: false,
        enabled: true,
        on_click: None,
    }
}

pub struct NativeButton {
    id: ElementId,
    title: String,
    symbol: Option<String>,
    symbol_only: bool,
    bezel: ButtonBezel,
    control_size: ControlSize,
    font_size: Option<Pixels>,
    weight: Option<FontWeight>,
    on: Option<bool>,
    border_on_hover: bool,
    square: Option<f32>,
    width: Option<Length>,
    default_button: bool,
    enabled: bool,
    on_click: Option<NativeHandler>,
}

impl NativeButton {
    /// SF Symbol on the face; the title is the accessibility name.
    pub fn symbol(mut self, name: impl Into<String>) -> Self {
        self.symbol = Some(name.into());
        self.symbol_only = true;
        self
    }

    pub fn trailing_symbol(mut self, name: impl Into<String>) -> Self {
        self.symbol = Some(name.into());
        self.symbol_only = false;
        self
    }

    pub fn bezel_style(mut self, bezel: ButtonBezel) -> Self {
        self.bezel = bezel;
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn font_size(mut self, font_size: Pixels) -> Self {
        self.font_size = Some(font_size);
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Push-on-push-off, drawn in this state; a click leaves the state to the next paint.
    pub fn on(mut self, on: bool) -> Self {
        self.on = Some(on);
        self
    }

    pub fn border_on_hover(mut self, border_on_hover: bool) -> Self {
        self.border_on_hover = border_on_hover;
        self
    }

    pub fn square(mut self, side: f32) -> Self {
        self.square = Some(side);
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Accent-tinted and activated by Return. At most one per window.
    pub fn default_button(mut self, default_button: bool) -> Self {
        self.default_button = default_button;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(move |_, window, cx| handler(&(), window, cx)));
        self
    }

    fn font(&self) -> Option<Retained<NSFont>> {
        if self.font_size.is_none() && self.weight.is_none() {
            return None;
        }
        let size = self
            .font_size
            .unwrap_or_else(|| system_font_size_for(self.control_size));
        let weight = ns_font_weight(self.weight.unwrap_or(FontWeight::NORMAL));
        Some(NSFont::systemFontOfSize_weight(
            f32::from(size) as f64,
            weight,
        ))
    }

    fn layout(&self) -> Style {
        if let Some(side) = self.square {
            return Style {
                size: size(px(side).into(), px(side).into()),
                ..Default::default()
            };
        }
        let inputs = (
            &self.title,
            &self.symbol,
            self.symbol_only,
            self.bezel,
            self.control_size,
            self.font_size.map(|size| f32::from(size).to_bits()),
            self.weight.map(|weight| weight.0.to_bits()),
        );
        let fitted = measured("NSButton.push", inputs, |mtm| {
            let make = NSButton::buttonWithTitle_target_action;
            let button = titled(mtm, make, &self.title, self.control_size);
            button.setBezelStyle(self.bezel.to_native());
            if let Some(font) = self.font() {
                button.setFont(Some(&font));
            }
            sync_symbol(
                &button,
                self.symbol.as_deref(),
                &self.title,
                self.symbol_only,
            );
            button.fittingSize()
        });
        with_width(fitted, self.width)
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let button = place_titled(
            &slot,
            NSButton::buttonWithTitle_target_action,
            &self.title,
            self.control_size,
        );
        let bezel = self.bezel.to_native();
        if button.bezelStyle() != bezel {
            button.setBezelStyle(bezel);
        }
        sync_symbol(
            &button,
            self.symbol.as_deref(),
            &self.title,
            self.symbol_only,
        );
        if let Some(font) = self.font() {
            if button.font().as_ref() != Some(&font) {
                button.setFont(Some(&font));
            }
        }
        if let Some(on) = self.on {
            let shows =
                NSCellStyleMask::ChangeGrayCellMask | NSCellStyleMask::ChangeBackgroundCellMask;
            let cell = button
                .cell()
                .and_then(|cell| cell.downcast::<NSButtonCell>().ok());
            if cell.is_some_and(|cell| cell.showsStateBy() != shows) {
                button.setButtonType(NSButtonType::PushOnPushOff);
            }
            let wanted = CheckState::from(on).to_native();
            if button.state() != wanted {
                button.setState(wanted);
            }
        }
        if button.showsBorderOnlyWhileMouseInside() != self.border_on_hover {
            button.setShowsBorderOnlyWhileMouseInside(self.border_on_hover);
        }
        if is_default_button(&button) != self.default_button {
            let wanted = if self.default_button { "\r" } else { "" };
            button.setKeyEquivalent(&NSString::from_str(wanted));
        }
        sync_enabled(&button, self.enabled);
        match self.on {
            Some(_) => {
                let clicked = self.on_click.take().map(|handler| -> NativeDispatch {
                    Rc::new(move |_, window, cx| handler(NativeValue::None, window, cx))
                });
                slot.on_event(unflip(), clicked);
            }
            None => slot.on_change(ValueKind::None, self.on_click.take()),
        }
    }
}

hosted_element!(NativeButton);

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum TrackingMode {
    #[default]
    SelectOne,
    /// Any number of segments can be on; `on_change` still reports a single index.
    SelectAny,
    Momentary,
}

impl TrackingMode {
    fn to_native(self) -> NSSegmentSwitchTracking {
        match self {
            Self::SelectOne => NSSegmentSwitchTracking::SelectOne,
            Self::SelectAny => NSSegmentSwitchTracking::SelectAny,
            Self::Momentary => NSSegmentSwitchTracking::Momentary,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum SegmentStyle {
    #[default]
    Automatic,
    Rounded,
    RoundRect,
    TexturedSquare,
    SmallSquare,
    Separated,
    TexturedRounded,
    Capsule,
}

impl SegmentStyle {
    fn to_native(self) -> NSSegmentStyle {
        match self {
            Self::Automatic => NSSegmentStyle::Automatic,
            Self::Rounded => NSSegmentStyle::Rounded,
            Self::RoundRect => NSSegmentStyle::RoundRect,
            Self::TexturedSquare => NSSegmentStyle::TexturedSquare,
            Self::SmallSquare => NSSegmentStyle::SmallSquare,
            Self::Separated => NSSegmentStyle::Separated,
            Self::TexturedRounded => NSSegmentStyle::TexturedRounded,
            Self::Capsule => NSSegmentStyle::Capsule,
        }
    }
}

pub fn native_segmented(id: impl Into<ElementId>, selected: usize) -> NativeSegmented {
    NativeSegmented {
        id: id.into(),
        selected,
        segments: Vec::new(),
        symbols: Vec::new(),
        tracking: TrackingMode::SelectOne,
        style: SegmentStyle::default(),
        control_size: ControlSize::default(),
        selected_bezel: None,
        enabled: true,
        segments_enabled: Vec::new(),
        on_change: None,
    }
}

pub struct NativeSegmented {
    id: ElementId,
    selected: usize,
    segments: Vec<String>,
    symbols: Vec<String>,
    tracking: TrackingMode,
    style: SegmentStyle,
    control_size: ControlSize,
    selected_bezel: Option<SystemColor>,
    enabled: bool,
    segments_enabled: Vec<bool>,
    on_change: Option<NativeHandler>,
}

impl NativeSegmented {
    pub fn segments(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.segments = segments.into_iter().map(Into::into).collect();
        self
    }

    /// SF Symbols on the face; `segments` stay the accessibility names.
    pub fn symbols(mut self, symbols: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.symbols = symbols.into_iter().map(Into::into).collect();
        self
    }

    pub fn tracking_mode(mut self, tracking: TrackingMode) -> Self {
        self.tracking = tracking;
        self
    }

    pub fn style(mut self, style: SegmentStyle) -> Self {
        self.style = style;
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn selected_bezel_color(mut self, color: SystemColor) -> Self {
        self.selected_bezel = Some(color);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn segment_enabled(mut self, segment: usize, enabled: bool) -> Self {
        if self.segments_enabled.len() <= segment {
            self.segments_enabled.resize(segment + 1, true);
        }
        self.segments_enabled[segment] = enabled;
        self
    }

    /// Reports the clicked segment, in `Momentary` too.
    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Int(raw) = value {
                if let Ok(index) = usize::try_from(raw) {
                    handler(&index, window, cx);
                }
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        let inputs = (&self.segments, &self.symbols, self.style, self.control_size);
        measured("NSSegmentedControl", inputs, |mtm| {
            let control = NSSegmentedControl::new(mtm);
            fill_segments(&control, &self.segments, &self.symbols);
            control.setSegmentStyle(self.style.to_native());
            control.setControlSize(self.control_size.into());
            control.fittingSize()
        })
    }

    fn paint(&mut self, mut slot: Slot, window: &mut Window, _: &mut App) {
        snap_origin(&mut slot, window.scale_factor());
        let control = slot.place(Layer::Above, |mtm, target| {
            let control = NSSegmentedControl::new(mtm);
            wire(&control, target, slot.key);
            control
        });
        fill_segments(&control, &self.segments, &self.symbols);
        let style = self.style.to_native();
        if control.segmentStyle() != style {
            control.setSegmentStyle(style);
        }
        sync_control_size(&control, self.control_size);
        let bezel = self.selected_bezel.map(SystemColor::ns_color);
        if control.selectedSegmentBezelColor() != bezel {
            control.setSelectedSegmentBezelColor(bezel.as_deref());
        }
        let tracking = self.tracking.to_native();
        if control.trackingMode() != tracking {
            control.setTrackingMode(tracking);
        }
        // Writing a selection in SelectAny pins that segment on; Momentary keeps none.
        if self.tracking == TrackingMode::SelectOne {
            let wanted = if self.selected < self.segments.len() {
                self.selected as isize
            } else {
                -1
            };
            if control.selectedSegment() != wanted {
                control.setSelectedSegment(wanted);
            }
        }
        sync_enabled(&control, self.enabled);
        for segment in 0..control.segmentCount() {
            let wanted = self
                .segments_enabled
                .get(segment as usize)
                .copied()
                .unwrap_or(true);
            if control.isEnabledForSegment(segment) != wanted {
                control.setEnabled_forSegment(wanted, segment);
            }
        }
        slot.on_change(ValueKind::Int, self.on_change.take());
    }
}

hosted_element!(NativeSegmented);

fn fill_segments(control: &NSSegmentedControl, segments: &[String], symbols: &[String]) {
    let count = segments.len().max(symbols.len());
    if control.segmentCount() != count as isize {
        control.setSegmentCount(count as isize);
    }
    for index in 0..count {
        let segment = index as isize;
        let label = segments.get(index).map_or("", String::as_str);
        let symbol = symbols.get(index).filter(|name| !name.is_empty());
        let image = symbol.and_then(|name| symbol_image(name, Some(label)));
        if control.imageForSegment(segment) != image {
            control.setImage_forSegment(image.as_deref(), segment);
        }
        let held = control.labelForSegment(segment);
        let (stale, face) = match symbol {
            Some(_) => (held.is_some_and(|held| !held.is_empty()), ""),
            None => (!held.is_some_and(|held| nsstring_is(&held, label)), label),
        };
        if stale {
            control.setLabel_forSegment(&NSString::from_str(face), segment);
        }
    }
}
