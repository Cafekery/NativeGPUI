//! The `NSButton` family — checkbox, radio, push button — and `NSSegmentedControl`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::Location;
use std::rc::Rc;
use std::sync::LazyLock;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Length, Pixels, Style, Window, px, relative, size,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControlStateValue, NSControlStateValueMixed, NSControlStateValueOff,
    NSControlStateValueOn, NSSegmentSwitchTracking, NSSegmentedControl,
};
use objc2_foundation::{MainThreadMarker, NSSize, NSString};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{
    Layer, NativeHandler, NativeHost, NativeValue, ValueKind, element_key, wire,
};

/// A preferred size cached per *content*, not per class: everything here is titled and the title
/// moves the size.
fn measured(
    class: &'static str,
    inputs: impl Hash,
    measure: impl FnOnce(MainThreadMarker) -> NSSize,
) -> NSSize {
    let mut hasher = DefaultHasher::new();
    inputs.hash(&mut hasher);
    fitting_size_keyed(class, hasher.finish(), measure)
}

/// A control with no intrinsic extent on an axis reports 0 for it, and that axis is GPUI's.
fn from_fitting(fitting: NSSize) -> Style {
    let axis = |reported: f64| -> Length {
        if reported > 0. {
            px(reported as f32).into()
        } else {
            relative(1.).into()
        }
    };
    let mut style = Style::default();
    style.size = size(axis(fitting.width), axis(fitting.height));
    style
}

/// `NG_BUTTON_TRACE=1` prints every property write.
static TRACE: LazyLock<bool> = LazyLock::new(|| std::env::var_os("NG_BUTTON_TRACE").is_some());

macro_rules! trace {
    ($($arg:tt)*) => {
        if *TRACE {
            println!("[ng-button] {}", format_args!($($arg)*));
        }
    };
}

/// An unconditional `setTitle:` re-lays-out the button's cell every frame.
fn sync_title(button: &NSButton, wanted: &str) -> bool {
    if button.title().to_string() == wanted {
        return false;
    }
    button.setTitle(&NSString::from_str(wanted));
    true
}

/// A checkbox's value. Mac checkboxes are tri-state, so this is not a `bool`.
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

/// A real macOS checkbox. `checked` is the value you own; wire [`NativeCheckbox::on_change`].
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

    /// Supersedes the `checked` bool. [`CheckState::Mixed`] implies [`Self::allows_mixed`].
    pub fn state(mut self, state: CheckState) -> Self {
        self.state = state;
        self.allows_mixed |= state == CheckState::Mixed;
        self
    }

    /// Also decides AppKit's click cycle — off, mixed, on — so leave it set for the control's
    /// lifetime rather than flipping it with the value.
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
}

impl Element for NativeCheckbox {
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
        let title = self.title.clone();
        let fitting = measured("NSButton.checkbox", &title, |mtm| unsafe {
            NSButton::checkboxWithTitle_target_action(&NSString::from_str(&title), None, None, mtm)
                .fittingSize()
        });
        (window.request_layout(from_fitting(fitting), [], cx), ())
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

        let title = self.title.clone();
        let button = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let button = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, key);
            trace!("checkbox {key:x} created");
            button
        });

        // `allowsMixedState` first: setting a mixed state on a two-state box is refused.
        if button.allowsMixedState() != self.allows_mixed {
            button.setAllowsMixedState(self.allows_mixed);
            trace!("checkbox {key:x} allowsMixedState <- {}", self.allows_mixed);
        }
        let wanted = self.state.to_native();
        if button.state() != wanted {
            button.setState(wanted);
            trace!("checkbox {key:x} state <- {:?}", self.state);
        }
        if sync_title(&button, &self.title) {
            trace!("checkbox {key:x} title <- {:?}", self.title);
        }
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::Int, self.on_change.take());
    }
}

impl IntoElement for NativeCheckbox {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// A real macOS radio button. The drawing is native; the grouping is ours — AppKit only
/// auto-groups radios sharing a superview and every hosted element has its own clip wrapper, so
/// hold the selected index yourself and set it from `on_change`, which only ever reports `true`.
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
}

impl Element for NativeRadio {
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
        let title = self.title.clone();
        let fitting = measured("NSButton.radio", &title, |mtm| unsafe {
            NSButton::radioButtonWithTitle_target_action(
                &NSString::from_str(&title),
                None,
                None,
                mtm,
            )
            .fittingSize()
        });
        (window.request_layout(from_fitting(fitting), [], cx), ())
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

        let title = self.title.clone();
        let button = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let button = unsafe {
                NSButton::radioButtonWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, key);
            trace!("radio {key:x} created");
            button
        });

        let wanted = CheckState::from(self.selected).to_native();
        if button.state() != wanted {
            button.setState(wanted);
            trace!("radio {key:x} state <- {}", self.selected);
        }
        if sync_title(&button, &self.title) {
            trace!("radio {key:x} title <- {:?}", self.title);
        }
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::Bool, self.on_change.take());
    }
}

impl IntoElement for NativeRadio {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Which bezel macOS should draw. `Automatic` lets AppKit pick from the button's contents and its
/// position in the window.
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

/// Asked of `NSWindow.defaultButtonCell`, not of `keyEquivalent()`: a button just set to `"\r"`
/// reads that property back as the empty string, so a diff against it would rewrite it forever.
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

    /// Accent-tinted by AppKit and activated by Return. Set it on at most one button per window —
    /// AppKit resolves `\r` to the first it finds.
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
}

impl Element for NativeButton {
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
        let title = self.title.clone();
        let bezel = self.bezel;
        let fitting = measured("NSButton.push", (&title, bezel), |mtm| unsafe {
            let button = NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&title),
                None,
                None,
                mtm,
            );
            button.setBezelStyle(bezel.to_native());
            button.fittingSize()
        });
        (window.request_layout(from_fitting(fitting), [], cx), ())
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

        let title = self.title.clone();
        let button = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(&title),
                    None,
                    None,
                    mtm,
                )
            };
            wire(&button, target, key);
            trace!("button {key:x} created");
            button
        });

        let bezel = self.bezel.to_native();
        if button.bezelStyle() != bezel {
            button.setBezelStyle(bezel);
            trace!("button {key:x} bezelStyle <- {:?}", self.bezel);
        }
        if sync_title(&button, &self.title) {
            trace!("button {key:x} title <- {:?}", self.title);
        }
        if is_default_button(&button) != self.default_button {
            let wanted = if self.default_button { "\r" } else { "" };
            button.setKeyEquivalent(&NSString::from_str(wanted));
            trace!("button {key:x} default <- {}", self.default_button);
        }
        if button.isEnabled() != self.enabled {
            button.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::None, self.on_click.take());
    }
}

impl IntoElement for NativeButton {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// How a segmented control tracks clicks.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum TrackingMode {
    /// Exactly one segment stays selected. The tab-strip behaviour.
    #[default]
    SelectOne,
    /// Any number of segments can be on at once. [`NativeSegmented::on_change`] reports a single
    /// index and cannot describe that, so prefer `SelectOne` or several `native_checkbox`es.
    SelectAny,
    /// Nothing stays selected; each click is a press. Selection is not written in this mode.
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

/// A real macOS `NSSegmentedControl`. `selected` is the index you own.
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

    /// Reports the clicked segment's index, including in [`TrackingMode::Momentary`] — the host
    /// reads it while the AppKit action is on the stack, before `selectedSegment` reverts to `-1`.
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

    /// `-1` is AppKit's "nothing selected"; 0 would select the first segment.
    fn wanted_selection(&self) -> isize {
        if self.selected < self.segments.len() {
            self.selected as isize
        } else {
            -1
        }
    }
}

impl Element for NativeSegmented {
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
        let segments = self.segments.clone();
        let fitting = measured("NSSegmentedControl", &segments, |mtm| {
            let control = NSSegmentedControl::new(mtm);
            control.setSegmentCount(segments.len() as isize);
            for (i, label) in segments.iter().enumerate() {
                control.setLabel_forSegment(&NSString::from_str(label), i as isize);
            }
            control.fittingSize()
        });
        (window.request_layout(from_fitting(fitting), [], cx), ())
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
            let control = NSSegmentedControl::new(mtm);
            wire(&control, target, key);
            trace!("segmented {key:x} created");
            control
        });

        let count = self.segments.len() as isize;
        if control.segmentCount() != count {
            control.setSegmentCount(count);
            trace!("segmented {key:x} segmentCount <- {count}");
        }
        // Per-segment compare rather than a rebuild: `setSegmentCount:` throws the labels away and
        // cancels any selection animation in flight.
        for (i, label) in self.segments.iter().enumerate() {
            let current = control.labelForSegment(i as isize).map(|s| s.to_string());
            if current.as_deref() != Some(label.as_str()) {
                control.setLabel_forSegment(&NSString::from_str(label), i as isize);
                trace!("segmented {key:x} label[{i}] <- {label:?}");
            }
        }

        let tracking = self.tracking.to_native();
        if control.trackingMode() != tracking {
            control.setTrackingMode(tracking);
            trace!("segmented {key:x} trackingMode <- {:?}", self.tracking);
        }
        // Only `SelectOne` has a selection a single index can describe. Momentary keeps none, and
        // in `SelectAny` a per-paint write pins the owned segment on: the user cannot switch it
        // off, and an out-of-range index deselects everything again every frame.
        if self.tracking == TrackingMode::SelectOne {
            let wanted = self.wanted_selection();
            if control.selectedSegment() != wanted {
                control.setSelectedSegment(wanted);
                trace!("segmented {key:x} selectedSegment <- {wanted}");
            }
        }
        if control.isEnabled() != self.enabled {
            control.setEnabled(self.enabled);
        }

        host.on_change(key, ValueKind::Int, self.on_change.take());
    }
}

impl IntoElement for NativeSegmented {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
