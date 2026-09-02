//! `NSButton` as checkbox, radio and push button, and `NSSegmentedControl`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use gpui::{App, ElementId, Style, Window};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControlStateValue, NSControlStateValueMixed, NSControlStateValueOff,
    NSControlStateValueOn, NSSegmentSwitchTracking, NSSegmentedControl,
};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{
    Layer, NativeHandler, NativeValue, Slot, ValueKind, hosted_element, wire,
};
use crate::controls::fitted;

/// A fitting size cached per content, since a title moves it.
fn measured(
    class: &'static str,
    inputs: impl Hash,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> Style {
    let mut hasher = DefaultHasher::new();
    inputs.hash(&mut hasher);
    fitted(fitting_size_keyed(class, hasher.finish(), measure))
}

fn sync_title(button: &NSButton, wanted: &str) {
    if button.title().to_string() != wanted {
        button.setTitle(&NSString::from_str(wanted));
    }
}

/// Mac checkboxes are tri-state.
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
        enabled: true,
        on_change: None,
    }
}

pub struct NativeCheckbox {
    id: ElementId,
    state: CheckState,
    allows_mixed: bool,
    title: String,
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
        let title = self.title.clone();
        measured("NSButton.checkbox", &title, |mtm| unsafe {
            NSButton::checkboxWithTitle_target_action(&NSString::from_str(&title), None, None, mtm)
                .fittingSize()
        })
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let title = self.title.clone();
        let button = slot.place(Layer::Above, |mtm, target| {
            let button = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, slot.key);
            button
        });
        // Before the state: a mixed state on a two-state box is refused.
        if button.allowsMixedState() != self.allows_mixed {
            button.setAllowsMixedState(self.allows_mixed);
        }
        let wanted = self.state.to_native();
        if button.state() != wanted {
            button.setState(wanted);
        }
        sync_title(&button, &self.title);
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }
        slot.on_change(ValueKind::Int, self.on_change.take());
    }
}

hosted_element!(NativeCheckbox);

/// AppKit only groups radios sharing a superview and every hosted view has its own, so hold the
/// selected index yourself; `on_change` only ever reports `true`.
pub fn native_radio(id: impl Into<ElementId>, selected: bool) -> NativeRadio {
    NativeRadio {
        id: id.into(),
        selected,
        title: String::new(),
        enabled: true,
        on_change: None,
    }
}

pub struct NativeRadio {
    id: ElementId,
    selected: bool,
    title: String,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeRadio {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Bool(on) = value {
                handler(&on, window, cx);
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        let title = self.title.clone();
        measured("NSButton.radio", &title, |mtm| unsafe {
            NSButton::radioButtonWithTitle_target_action(
                &NSString::from_str(&title),
                None,
                None,
                mtm,
            )
            .fittingSize()
        })
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let title = self.title.clone();
        let button = slot.place(Layer::Above, |mtm, target| {
            let button = unsafe {
                NSButton::radioButtonWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, slot.key);
            button
        });
        let wanted = CheckState::from(self.selected).to_native();
        if button.state() != wanted {
            button.setState(wanted);
        }
        sync_title(&button, &self.title);
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }
        slot.on_change(ValueKind::Bool, self.on_change.take());
    }
}

hosted_element!(NativeRadio);

/// `Automatic` lets AppKit pick from the button's contents and position.
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
    fn to_native(self) -> NSBezelStyle {
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
        (Some(cell), Some(default)) => std::ptr::eq(
            Retained::as_ptr(&cell).cast::<AnyObject>(),
            Retained::as_ptr(&default).cast::<AnyObject>(),
        ),
        _ => false,
    }
}

pub fn native_button(id: impl Into<ElementId>, title: impl Into<String>) -> NativeButton {
    NativeButton {
        id: id.into(),
        title: title.into(),
        bezel: ButtonBezel::Automatic,
        default_button: false,
        enabled: true,
        on_click: None,
    }
}

pub struct NativeButton {
    id: ElementId,
    title: String,
    bezel: ButtonBezel,
    default_button: bool,
    enabled: bool,
    on_click: Option<NativeHandler>,
}

impl NativeButton {
    pub fn bezel_style(mut self, bezel: ButtonBezel) -> Self {
        self.bezel = bezel;
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

    fn layout(&self) -> Style {
        let (title, bezel) = (self.title.clone(), self.bezel);
        measured("NSButton.push", (&title, bezel), |mtm| unsafe {
            let button = NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&title),
                None,
                None,
                mtm,
            );
            button.setBezelStyle(bezel.to_native());
            button.fittingSize()
        })
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let title = self.title.clone();
        let button = slot.place(Layer::Above, |mtm, target| {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, slot.key);
            button
        });
        let bezel = self.bezel.to_native();
        if button.bezelStyle() != bezel {
            button.setBezelStyle(bezel);
        }
        sync_title(&button, &self.title);
        if is_default_button(&button) != self.default_button {
            let wanted = if self.default_button { "\r" } else { "" };
            button.setKeyEquivalent(&NSString::from_str(wanted));
        }
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }
        slot.on_change(ValueKind::None, self.on_click.take());
    }
}

hosted_element!(NativeButton);

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum TrackingMode {
    /// Exactly one segment stays selected.
    #[default]
    SelectOne,
    /// Any number of segments can be on; `on_change` still reports a single index.
    SelectAny,
    /// Nothing stays selected; each click is a press.
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

pub fn native_segmented(id: impl Into<ElementId>, selected: usize) -> NativeSegmented {
    NativeSegmented {
        id: id.into(),
        selected,
        segments: Vec::new(),
        tracking: TrackingMode::SelectOne,
        enabled: true,
        on_change: None,
    }
}

pub struct NativeSegmented {
    id: ElementId,
    selected: usize,
    segments: Vec<String>,
    tracking: TrackingMode,
    enabled: bool,
    on_change: Option<NativeHandler>,
}

impl NativeSegmented {
    pub fn segments(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.segments = segments.into_iter().map(Into::into).collect();
        self
    }

    pub fn tracking_mode(mut self, tracking: TrackingMode) -> Self {
        self.tracking = tracking;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Reports the clicked segment, in `Momentary` too: it is read before `selectedSegment`
    /// reverts to -1.
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
        let segments = self.segments.clone();
        measured("NSSegmentedControl", &segments, |mtm| {
            let control = NSSegmentedControl::new(mtm);
            control.setSegmentCount(segments.len() as isize);
            for (i, label) in segments.iter().enumerate() {
                control.setLabel_forSegment(&NSString::from_str(label), i as isize);
            }
            control.fittingSize()
        })
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let control = slot.place(Layer::Above, |mtm, target| {
            let control = NSSegmentedControl::new(mtm);
            wire(&control, target, slot.key);
            control
        });
        let count = self.segments.len() as isize;
        if control.segmentCount() != count {
            control.setSegmentCount(count);
        }
        for (i, label) in self.segments.iter().enumerate() {
            let current = control.labelForSegment(i as isize).map(|s| s.to_string());
            if current.as_deref() != Some(label.as_str()) {
                control.setLabel_forSegment(&NSString::from_str(label), i as isize);
            }
        }
        let tracking = self.tracking.to_native();
        if control.trackingMode() != tracking {
            control.setTrackingMode(tracking);
        }
        // Only SelectOne has a selection one index describes; writing it in SelectAny pins a
        // segment on, and Momentary keeps none.
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
        if control.isEnabled() != self.enabled {
            control.setEnabled(self.enabled);
        }
        slot.on_change(ValueKind::Int, self.on_change.take());
    }
}

hosted_element!(NativeSegmented);
