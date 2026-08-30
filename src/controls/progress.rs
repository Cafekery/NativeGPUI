//! `NSProgressIndicator` (bar and spinner) and `NSLevelIndicator`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Length, Pixels, Style, Window, px, relative, size,
};
use objc2_app_kit::{
    NSControlSize, NSLevelIndicator, NSLevelIndicatorStyle, NSProgressIndicator,
    NSProgressIndicatorStyle,
};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{
    ControlSize, Layer, NativeHandler, NativeHost, NativeValue, ValueKind, element_key, wire,
};

/// How an `NSLevelIndicator` presents its value.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LevelStyle {
    /// Stars. The interactive one: pair with [`NativeLevel::editable`].
    #[default]
    Rating,
    /// A solid capacity bar that turns yellow past the warning value and red past critical.
    ContinuousCapacity,
    /// The same, drawn as discrete segments.
    DiscreteCapacity,
    /// The dotted relevancy meter Spotlight uses.
    Relevancy,
}

impl LevelStyle {
    fn appkit(self) -> NSLevelIndicatorStyle {
        match self {
            LevelStyle::Rating => NSLevelIndicatorStyle::Rating,
            LevelStyle::ContinuousCapacity => NSLevelIndicatorStyle::ContinuousCapacity,
            LevelStyle::DiscreteCapacity => NSLevelIndicatorStyle::DiscreteCapacity,
            LevelStyle::Relevancy => NSLevelIndicatorStyle::Relevancy,
        }
    }
}

/// `fraction` is 0.0..=1.0, and is ignored while [`NativeProgress::indeterminate`] is set.
pub fn native_progress(id: impl Into<ElementId>, fraction: f64) -> NativeProgress {
    NativeProgress {
        id: id.into(),
        fraction,
        indeterminate: false,
        control_size: ControlSize::default(),
        width: relative(1.).into(),
    }
}

pub struct NativeProgress {
    id: ElementId,
    fraction: f64,
    indeterminate: bool,
    control_size: ControlSize,
    width: Length,
}

impl NativeProgress {
    /// Switch to the animated barber-pole.
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }
    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }
    /// The free axis. Defaults to the full width of the parent.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
}

fn bar_thickness(control_size: ControlSize) -> f64 {
    let appkit = NSControlSize::from(control_size);
    fitting_size_keyed("NSProgressIndicator.bar", appkit.0 as u64, |mtm| {
        let bar = NSProgressIndicator::new(mtm);
        bar.setStyle(NSProgressIndicatorStyle::Bar);
        bar.setControlSize(appkit);
        bar.sizeToFit();
        bar.fittingSize()
    })
    .height
}

impl Element for NativeProgress {
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
        let mut style = Style::default();
        style.size = size(
            self.width,
            px(bar_thickness(self.control_size) as f32).into(),
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
        let indeterminate = self.indeterminate;

        let bar = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            let bar = NSProgressIndicator::new(mtm);
            bar.setStyle(NSProgressIndicatorStyle::Bar);
            bar.setMinValue(0.);
            bar.setMaxValue(1.);
            bar.setIndeterminate(indeterminate);
            if indeterminate {
                unsafe { bar.startAnimation(None) };
            }
            bar
        });

        if bar.controlSize() != NSControlSize::from(self.control_size) {
            bar.setControlSize(NSControlSize::from(self.control_size));
        }
        // Animation follows the transition, never the frame: re-issuing `startAnimation:` every
        // frame restarts the barber-pole from zero, which looks static.
        if bar.isIndeterminate() != indeterminate {
            bar.setIndeterminate(indeterminate);
            unsafe {
                if indeterminate {
                    bar.startAnimation(None);
                } else {
                    bar.stopAnimation(None);
                }
            }
        }
        if !indeterminate {
            let wanted = self.fraction.clamp(0., 1.);
            if bar.doubleValue() != wanted {
                bar.setDoubleValue(wanted);
            }
        }
    }
}

impl IntoElement for NativeProgress {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// The circular indeterminate `NSProgressIndicator`. Always animating.
pub fn native_spinner(id: impl Into<ElementId>) -> NativeSpinner {
    NativeSpinner {
        id: id.into(),
        control_size: ControlSize::default(),
    }
}

pub struct NativeSpinner {
    id: ElementId,
    control_size: ControlSize,
}

impl NativeSpinner {
    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }
}

fn spinner_size(control_size: ControlSize) -> objc2_foundation::NSSize {
    let appkit = NSControlSize::from(control_size);
    fitting_size_keyed("NSProgressIndicator.spinning", appkit.0 as u64, |mtm| {
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(appkit);
        spinner.sizeToFit();
        spinner.fittingSize()
    })
}

impl Element for NativeSpinner {
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
        let fitting = spinner_size(self.control_size);
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
        let control_size = NSControlSize::from(self.control_size);

        let spinner = host.place(key, Layer::Above, bounds, mask, |mtm, _| {
            let spinner = NSProgressIndicator::new(mtm);
            spinner.setStyle(NSProgressIndicatorStyle::Spinning);
            spinner.setIndeterminate(true);
            spinner.setControlSize(control_size);
            spinner.setDisplayedWhenStopped(true);
            unsafe { spinner.startAnimation(None) };
            spinner
        });

        if spinner.controlSize() != control_size {
            spinner.setControlSize(control_size);
        }
    }
}

impl IntoElement for NativeSpinner {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// An `NSLevelIndicator`: stars, a capacity meter, or a relevancy meter. With
/// [`NativeLevel::editable`] the rating style is interactive.
pub fn native_level(id: impl Into<ElementId>, value: f64) -> NativeLevel {
    NativeLevel {
        id: id.into(),
        value,
        style: LevelStyle::Rating,
        min: 0.,
        max: 5.,
        warning: None,
        critical: None,
        editable: false,
        enabled: true,
        width: None,
        on_change: None,
    }
}

pub struct NativeLevel {
    id: ElementId,
    value: f64,
    style: LevelStyle,
    min: f64,
    max: f64,
    warning: Option<f64>,
    critical: Option<f64>,
    editable: bool,
    enabled: bool,
    width: Option<Length>,
    on_change: Option<NativeHandler>,
}

impl NativeLevel {
    pub fn style(mut self, style: LevelStyle) -> Self {
        self.style = style;
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }
    pub fn warning_value(mut self, warning: f64) -> Self {
        self.warning = Some(warning);
        self
    }
    pub fn critical_value(mut self, critical: f64) -> Self {
        self.critical = Some(critical);
        self
    }
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Only meaningful for the styles AppKit reports no intrinsic width for — the capacity and
    /// relevancy meters.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
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
}

fn level_size(style: LevelStyle, min: f64, max: f64) -> objc2_foundation::NSSize {
    let appkit = style.appkit();
    let mut hash = DefaultHasher::new();
    (appkit.0, min.to_bits(), max.to_bits()).hash(&mut hash);
    fitting_size_keyed("NSLevelIndicator", hash.finish(), |mtm| {
        let probe = NSLevelIndicator::new(mtm);
        probe.setLevelIndicatorStyle(appkit);
        probe.setMinValue(min);
        probe.setMaxValue(max);
        probe.fittingSize()
    })
}

impl Element for NativeLevel {
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
        let fitting = level_size(self.style, self.min, self.max);
        // A zero axis from `fittingSize` means AppKit declares that axis free: a rating knows
        // how wide a star is, the capacity and relevancy styles stretch.
        let width = self.width.unwrap_or(if fitting.width > 0. {
            px(fitting.width as f32).into()
        } else {
            relative(1.).into()
        });
        let mut style = Style::default();
        style.size = size(width, px(fitting.height as f32).into());
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

        let level = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
            let level = NSLevelIndicator::new(mtm);
            wire(&level, target, key);
            level
        });

        let style = self.style.appkit();
        if level.levelIndicatorStyle() != style {
            level.setLevelIndicatorStyle(style);
        }
        if level.minValue() != self.min {
            level.setMinValue(self.min);
        }
        if level.maxValue() != self.max {
            level.setMaxValue(self.max);
        }
        // Warning and critical default to the max, which is AppKit's own "unset".
        let warning = self.warning.unwrap_or(self.max);
        if level.warningValue() != warning {
            level.setWarningValue(warning);
        }
        let critical = self.critical.unwrap_or(self.max);
        if level.criticalValue() != critical {
            level.setCriticalValue(critical);
        }
        if level.isEditable() != self.editable {
            level.setEditable(self.editable);
            // Without continuous, the action only fires on mouse-up and a drag across the stars
            // does not track.
            level.setContinuous(self.editable);
        }
        if level.isEnabled() != self.enabled {
            level.setEnabled(self.enabled);
        }
        let wanted = self.value.clamp(self.min, self.max);
        if level.doubleValue() != wanted {
            level.setDoubleValue(wanted);
        }

        host.on_change(key, ValueKind::Number, self.on_change.take());
    }
}

impl IntoElement for NativeLevel {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
