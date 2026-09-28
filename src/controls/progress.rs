use gpui::{App, ElementId, Length, Style, Window, px, relative, size};
use objc2_app_kit::{
    NSControlSize, NSLevelIndicator, NSLevelIndicatorStyle, NSProgressIndicator,
    NSProgressIndicatorStyle,
};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{
    ControlSize, Layer, NativeHandler, Slot, ValueKind, hosted_element, wire,
};
use crate::controls::{axis, fitted, hash_of, number_handler, sync_enabled};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LevelStyle {
    #[default]
    Rating,
    ContinuousCapacity,
    DiscreteCapacity,
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
    pub fn indeterminate(mut self, indeterminate: bool) -> Self {
        self.indeterminate = indeterminate;
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    fn layout(&self) -> Style {
        let appkit = NSControlSize::from(self.control_size);
        let fitting = fitting_size_keyed("NSProgressIndicator.bar", appkit.0 as u64, |mtm| {
            let bar = NSProgressIndicator::new(mtm);
            bar.setStyle(NSProgressIndicatorStyle::Bar);
            bar.setControlSize(appkit);
            bar.sizeToFit();
            bar.fittingSize()
        });
        Style {
            size: size(self.width, px(fitting.height as f32).into()),
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let indeterminate = self.indeterminate;
        let bar = slot.place(Layer::Above, |mtm, _| {
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
        let control_size = NSControlSize::from(self.control_size);
        if bar.controlSize() != control_size {
            bar.setControlSize(control_size);
        }
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

hosted_element!(NativeProgress);

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

    fn layout(&self) -> Style {
        let appkit = NSControlSize::from(self.control_size);
        fitted(fitting_size_keyed(
            "NSProgressIndicator.spinning",
            appkit.0 as u64,
            |mtm| {
                let spinner = NSProgressIndicator::new(mtm);
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(appkit);
                spinner.sizeToFit();
                spinner.fittingSize()
            },
        ))
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let control_size = NSControlSize::from(self.control_size);
        let spinner = slot.place(Layer::Above, |mtm, _| {
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

hosted_element!(NativeSpinner);

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

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(number_handler(handler));
        self
    }

    fn layout(&self) -> Style {
        let (appkit, min, max) = (self.style.appkit(), self.min, self.max);
        let key = hash_of((appkit.0, min.to_bits(), max.to_bits()));
        let fitting = fitting_size_keyed("NSLevelIndicator", key, |mtm| {
            let probe = NSLevelIndicator::new(mtm);
            probe.setLevelIndicatorStyle(appkit);
            probe.setMinValue(min);
            probe.setMaxValue(max);
            probe.fittingSize()
        });
        Style {
            size: size(
                self.width.unwrap_or_else(|| axis(fitting.width)),
                px(fitting.height as f32).into(),
            ),
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let level = slot.place(Layer::Above, |mtm, target| {
            let level = NSLevelIndicator::new(mtm);
            wire(&level, target, slot.key);
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
            level.setContinuous(self.editable);
        }
        sync_enabled(&level, self.enabled);
        let wanted = self.value.clamp(self.min, self.max);
        if level.doubleValue() != wanted {
            level.setDoubleValue(wanted);
        }
        slot.on_change(ValueKind::Number, self.on_change.take());
    }
}

hosted_element!(NativeLevel);
