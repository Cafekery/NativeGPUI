use std::cell::Cell;
use std::ffi::CStr;
use std::sync::LazyLock;

use gpui::{App, ElementId, Hsla, Pixels, Point, Size, Style, Window, point, relative};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{NSGlassEffectView, NSView};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNumber, NSPoint, NSRect, NSSize, NSString, NSValue, ns_string,
};
use objc2_quartz_core::{CALayer, CATransaction, kCACornerCurveContinuous};

use crate::bridge::host::{Layer, Slot, hosted_element};
use crate::system::color::to_nscolor;

/// Filter inputs that move nothing on macOS 26.5, kept so a later OS can be re-checked.
pub const INERT_INPUTS: [&str; 8] = [
    "inputBlurOpacity3",
    "inputBlurOpacity4",
    "inputBlurDistance0",
    "inputBlurDistance4",
    "inputClampPreserveHue",
    "inputBlurDistance1",
    "inputClamp",
    "inputShadowDistanceOffset",
];

/// Every live input of the private glass filter plus the geometry of the field it reads. Nothing
/// clamps; the presets are the calibrated end of the range. A `None` fill means no fill colour.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct GlassRecipe {
    pub corner_radius: f32,
    pub ovalization: f32,
    /// How far outside its bounds the backdrop samples.
    pub margin_width: f32,
    /// How far the backdrop downsamples before the filter reads it.
    pub backdrop_scale: f32,

    pub blur_radius: f32,
    pub blur_opacity0: f32,
    pub blur_opacity1: f32,
    pub blur_opacity2: f32,
    pub blur_distance2: f32,
    pub blur_distance3: f32,

    pub inner_refraction_amount: f32,
    pub inner_refraction_height: f32,
    pub outer_refraction_amount: f32,
    pub outer_refraction_height: f32,
    /// Gates both refraction distances.
    pub refraction_opacity: f32,
    pub refraction_distance0: f32,
    pub refraction_distance1: f32,

    pub face_opacity: f32,
    pub face_white: f32,
    pub face_black: f32,
    pub face_saturation: f32,
    pub face_fill: Option<Hsla>,

    /// Gates every other bleed input.
    pub bleed_opacity: f32,
    pub bleed_amount: f32,
    pub bleed_height: f32,
    pub bleed_blur_radius: f32,
    pub bleed_distance0: f32,
    pub bleed_distance1: f32,
    pub bleed_darken_blend: bool,
    pub bleed_white: f32,
    pub bleed_black: f32,
    pub bleed_saturation: f32,
    pub bleed_fill: Option<Hsla>,

    pub shadow_radius: f32,
    pub shadow_offset: Point<f32>,
    pub shadow_amount: f32,
    pub shadow_height: f32,
    pub shadow_blur_radius: f32,
    pub shadow_opacity: f32,
    /// The dominant one on an SDR display.
    pub shadow_opacity_sdr: f32,
    pub shadow_vibrancy: f32,
    pub shadow_white: f32,
    pub shadow_black: f32,
    pub shadow_saturation: f32,
    pub shadow_fill: Option<Hsla>,

    /// Gates the four fields below it.
    pub holding_tone: bool,
    pub holding_tone_white: f32,
    pub max_headroom: f32,
    pub gradient_distance0: f32,
    pub gradient_distance1: f32,
}

impl Default for GlassRecipe {
    fn default() -> Self {
        Self::regular()
    }
}

impl GlassRecipe {
    /// What a stock `NSGlassEffectView` in a key window writes into its filter, dark appearance.
    pub fn regular() -> Self {
        Self {
            corner_radius: 0.,
            ovalization: 0.5,
            margin_width: 83.,
            backdrop_scale: 0.25,
            blur_radius: 4.,
            blur_opacity0: 1.,
            blur_opacity1: 0.5,
            blur_opacity2: 0.5,
            blur_distance2: 0.,
            blur_distance3: 0.,
            inner_refraction_amount: -60.,
            inner_refraction_height: 20.,
            outer_refraction_amount: 40.,
            outer_refraction_height: 25.,
            refraction_opacity: 0.3,
            refraction_distance0: -1.,
            refraction_distance1: -0.5,
            face_opacity: 1.,
            face_white: 0.6,
            face_black: 0.2,
            face_saturation: 1.,
            face_fill: Some(grey(0., 0.4)),
            bleed_opacity: 0.8,
            bleed_amount: 70.,
            bleed_height: 70.,
            bleed_blur_radius: 140.,
            bleed_distance0: 1.,
            bleed_distance1: 0.,
            bleed_darken_blend: false,
            bleed_white: 0.5,
            bleed_black: 0.,
            bleed_saturation: 1.,
            bleed_fill: None,
            shadow_radius: 24.,
            shadow_offset: point(0., 8.),
            shadow_amount: 75.,
            shadow_height: 80.,
            shadow_blur_radius: 40.,
            shadow_opacity: 0.25,
            shadow_opacity_sdr: 0.24,
            shadow_vibrancy: 1.,
            shadow_white: 0.5,
            shadow_black: 0.,
            shadow_saturation: 1.,
            shadow_fill: None,
            holding_tone: true,
            holding_tone_white: 0.97,
            max_headroom: 9999.,
            gradient_distance0: -1.,
            gradient_distance1: -0.5,
        }
    }

    pub fn regular_light() -> Self {
        Self {
            face_white: 1.03,
            face_black: 0.5,
            face_fill: Some(grey(1., 0.4)),
            bleed_opacity: 0.5,
            bleed_darken_blend: true,
            bleed_white: 1.,
            bleed_black: 0.9,
            bleed_saturation: 1.2,
            shadow_white: 1.,
            shadow_saturation: 1.8,
            shadow_fill: Some(grey(0., 0.12)),
            ..Self::regular()
        }
    }

    /// The clear style: tints rather than blurs; the same in both appearances.
    pub fn clear() -> Self {
        Self {
            ovalization: 0.,
            margin_width: 0.,
            backdrop_scale: 0.5,
            blur_radius: 1.,
            refraction_opacity: 0.,
            face_white: 1.15,
            face_black: 0.075,
            face_saturation: 1.06,
            face_fill: Some(grey(1., 0.)),
            bleed_opacity: 0.,
            bleed_amount: 0.,
            bleed_height: 0.,
            bleed_blur_radius: 0.,
            bleed_darken_blend: true,
            bleed_white: 1.,
            bleed_black: 0.75,
            bleed_saturation: 1.2,
            shadow_radius: 0.,
            shadow_blur_radius: 0.,
            shadow_opacity: 0.,
            shadow_vibrancy: 0.,
            shadow_white: 1.,
            shadow_saturation: 1.2,
            shadow_fill: Some(grey(0., 0.1)),
            ..Self::regular()
        }
    }

    pub fn blob() -> Self {
        Self {
            corner_radius: 300.,
            blur_radius: 0.,
            inner_refraction_amount: -300.,
            inner_refraction_height: 100.,
            refraction_distance0: 0.,
            refraction_distance1: 0.,
            face_white: 0.97,
            face_black: 0.05,
            bleed_amount: 0.,
            bleed_height: 0.,
            bleed_white: 1.,
            bleed_darken_blend: true,
            shadow_radius: 180.,
            shadow_opacity_sdr: 0.2,
            shadow_fill: Some(grey(0., 0.12)),
            holding_tone: true,
            holding_tone_white: 0.99,
            ..Self::regular()
        }
    }
}

fn grey(level: f32, alpha: f32) -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: level,
        a: alpha,
    }
}

/// Whether the three layer classes and `CAFilter` all resolved.
pub fn custom_glass_available() -> bool {
    private_class(c"CABackdropLayer").is_some()
        && private_class(c"CASDFLayer").is_some()
        && private_class(c"CASDFElementLayer").is_some()
        && private_class(c"CAFilter").is_some()
}

/// A panel of liquid glass drawn from a [`GlassRecipe`], filling the box it is given.
pub fn custom_glass(id: impl Into<ElementId>) -> CustomGlass {
    CustomGlass {
        id: id.into(),
        layer: Layer::Above,
        recipe: GlassRecipe::regular(),
        radius: None,
    }
}

pub struct CustomGlass {
    id: ElementId,
    layer: Layer,
    recipe: GlassRecipe,
    radius: Option<f32>,
}

impl CustomGlass {
    pub fn frosting_gpui(mut self) -> Self {
        self.layer = Layer::Above;
        self
    }

    pub fn frosting_desktop(mut self) -> Self {
        self.layer = Layer::Below;
        self
    }

    pub fn recipe(mut self, recipe: GlassRecipe) -> Self {
        self.recipe = recipe;
        self
    }

    /// Overrides the recipe's radius, in either order.
    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.radius = Some(radius.to_f64() as f32);
        self
    }

    fn layout(&self) -> Style {
        Style {
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Default::default()
        }
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let view = slot.place(self.layer, |mtm, _| CustomGlassView::new(mtm));
        let mut recipe = self.recipe;
        if let Some(radius) = self.radius {
            recipe.corner_radius = radius;
        }
        view.fit();
        view.apply(&recipe);
    }
}

hosted_element!(CustomGlass);

enum Body {
    Layers {
        backdrop: Retained<CALayer>,
        field: Retained<CALayer>,
        element: Retained<CALayer>,
    },
    Stock(Retained<NSGlassEffectView>),
    Absent,
}

struct GlassIvars {
    body: Body,
    applied: Cell<Option<GlassRecipe>>,
    size: Cell<NSSize>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGCustomGlass"]
    #[ivars = GlassIvars]
    struct CustomGlassView;
);

/// The filter reads the field by layer name.
fn field_name() -> &'static NSString {
    ns_string!("ngGlassField")
}

impl CustomGlassView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let ivars = GlassIvars {
            body: Body::new(mtm),
            applied: Cell::new(None),
            size: Cell::new(NSSize::ZERO),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setWantsLayer(true);
        let root = this.layer().expect("wantsLayer was just set");
        match &this.ivars().body {
            Body::Layers { backdrop, .. } => root.addSublayer(backdrop),
            Body::Stock(view) => this.addSubview(view),
            Body::Absent => {}
        }
        this
    }

    fn apply(&self, recipe: &GlassRecipe) {
        let ivars = self.ivars();
        if ivars.applied.get().as_ref() == Some(recipe) {
            return;
        }
        ivars.applied.set(Some(*recipe));
        without_animation(|| match &ivars.body {
            Body::Layers {
                backdrop, element, ..
            } => {
                element.setCornerRadius(recipe.corner_radius as f64);
                set_number(
                    &**element,
                    ns_string!("gradientOvalization"),
                    recipe.ovalization,
                );
                set_number(&**backdrop, ns_string!("marginWidth"), recipe.margin_width);
                set_number(&**backdrop, ns_string!("scale"), recipe.backdrop_scale);
                // `CAFilter` ignores writes after its first commit, so a change ships a new one.
                let filter = build_filter(recipe);
                let filters = filter.map(|filter| NSArray::from_retained_slice(&[filter]));
                unsafe { backdrop.setFilters(filters.as_deref()) };
            }
            Body::Stock(view) => view.setCornerRadius(recipe.corner_radius as f64),
            Body::Absent => {}
        });
    }

    /// Called from `paint`: the pool sets geometry with `setFrame:`, which does not reach
    /// `setFrameSize:`.
    fn fit(&self) {
        let ivars = self.ivars();
        let size = self.bounds().size;
        if ivars.size.get() == size {
            return;
        }
        ivars.size.set(size);
        let frame = NSRect::new(NSPoint::ZERO, size);
        without_animation(|| match &ivars.body {
            Body::Layers {
                backdrop,
                field,
                element,
            } => {
                backdrop.setFrame(frame);
                field.setFrame(frame);
                element.setFrame(frame);
            }
            Body::Stock(view) => view.setFrame(frame),
            Body::Absent => {}
        });
    }
}

impl Body {
    fn new(mtm: MainThreadMarker) -> Self {
        if let (Some(backdrop), Some(field), Some(element)) = (
            private_layer(c"CABackdropLayer"),
            private_layer(c"CASDFLayer"),
            private_layer(c"CASDFElementLayer"),
        ) {
            set_bool(&*backdrop, ns_string!("allowsFilteredLuma"), true);
            field.setName(Some(field_name()));
            element.setCornerCurve(unsafe { kCACornerCurveContinuous });
            // The output effect is what makes the field render at all.
            if let Some(effect) = private_instance(c"CASDFOutputEffect") {
                set_object(&*field, ns_string!("effect"), Some(&*effect));
            }
            field.addSublayer(&element);
            backdrop.addSublayer(&field);
            return Self::Layers {
                backdrop,
                field,
                element,
            };
        }
        match AnyClass::get(c"NSGlassEffectView") {
            Some(_) => Self::Stock(NSGlassEffectView::new(mtm)),
            None => Self::Absent,
        }
    }
}

fn build_filter(recipe: &GlassRecipe) -> Option<Retained<AnyObject>> {
    let class = private_class(c"CAFilter")?;
    let filter: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, filterWithType: ns_string!("glassBackground")] };
    let filter = filter?;
    let num = |key: &NSString, value: f32| set_number(&*filter, key, value);
    let flag = |key: &NSString, value: bool| set_bool(&*filter, key, value);
    let fill = |key: &NSString, value: Option<Hsla>| set_fill(&*filter, key, value);

    set_object(
        &*filter,
        ns_string!("inputSourceSublayerName"),
        Some(field_name()),
    );

    num(ns_string!("inputBlurRadius"), recipe.blur_radius);
    num(ns_string!("inputBlurOpacity0"), recipe.blur_opacity0);
    num(ns_string!("inputBlurOpacity1"), recipe.blur_opacity1);
    num(ns_string!("inputBlurOpacity2"), recipe.blur_opacity2);
    num(ns_string!("inputBlurDistance2"), recipe.blur_distance2);
    num(ns_string!("inputBlurDistance3"), recipe.blur_distance3);

    num(
        ns_string!("inputInnerRefractionAmount"),
        recipe.inner_refraction_amount,
    );
    num(
        ns_string!("inputInnerRefractionHeight"),
        recipe.inner_refraction_height,
    );
    num(
        ns_string!("inputOuterRefractionAmount"),
        recipe.outer_refraction_amount,
    );
    num(
        ns_string!("inputOuterRefractionHeight"),
        recipe.outer_refraction_height,
    );
    num(
        ns_string!("inputRefractionOpacity"),
        recipe.refraction_opacity,
    );
    num(
        ns_string!("inputRefractionDistance0"),
        recipe.refraction_distance0,
    );
    num(
        ns_string!("inputRefractionDistance1"),
        recipe.refraction_distance1,
    );

    num(ns_string!("inputFaceOpacity"), recipe.face_opacity);
    num(ns_string!("inputFaceColorMatrixWhite"), recipe.face_white);
    num(ns_string!("inputFaceColorMatrixBlack"), recipe.face_black);
    num(
        ns_string!("inputFaceColorMatrixSaturation"),
        recipe.face_saturation,
    );
    fill(
        ns_string!("inputFaceColorMatrixFillColor"),
        recipe.face_fill,
    );

    num(ns_string!("inputBleedOpacity"), recipe.bleed_opacity);
    num(ns_string!("inputBleedAmount"), recipe.bleed_amount);
    num(ns_string!("inputBleedHeight"), recipe.bleed_height);
    num(ns_string!("inputBleedBlurRadius"), recipe.bleed_blur_radius);
    num(ns_string!("inputBleedDistance0"), recipe.bleed_distance0);
    num(ns_string!("inputBleedDistance1"), recipe.bleed_distance1);
    flag(
        ns_string!("inputBleedDarkenBlend"),
        recipe.bleed_darken_blend,
    );
    num(ns_string!("inputBleedColorMatrixWhite"), recipe.bleed_white);
    num(ns_string!("inputBleedColorMatrixBlack"), recipe.bleed_black);
    num(
        ns_string!("inputBleedColorMatrixSaturation"),
        recipe.bleed_saturation,
    );
    fill(
        ns_string!("inputBleedColorMatrixFillColor"),
        recipe.bleed_fill,
    );

    num(ns_string!("inputShadowRadius"), recipe.shadow_radius);
    num(ns_string!("inputShadowAmount"), recipe.shadow_amount);
    num(ns_string!("inputShadowHeight"), recipe.shadow_height);
    num(
        ns_string!("inputShadowBlurRadius"),
        recipe.shadow_blur_radius,
    );
    set_size(
        &*filter,
        ns_string!("inputShadowOffset"),
        recipe.shadow_offset,
    );
    num(ns_string!("inputShadowOpacity"), recipe.shadow_opacity);
    num(
        ns_string!("inputSDRShadowOpacity"),
        recipe.shadow_opacity_sdr,
    );
    num(
        ns_string!("inputShadowVibrancyContribution"),
        recipe.shadow_vibrancy,
    );
    num(
        ns_string!("inputShadowColorMatrixWhite"),
        recipe.shadow_white,
    );
    num(
        ns_string!("inputShadowColorMatrixBlack"),
        recipe.shadow_black,
    );
    num(
        ns_string!("inputShadowColorMatrixSaturation"),
        recipe.shadow_saturation,
    );
    fill(
        ns_string!("inputShadowColorMatrixFillColor"),
        recipe.shadow_fill,
    );

    flag(
        ns_string!("inputSDRHoldingToneEnabled"),
        recipe.holding_tone,
    );
    num(
        ns_string!("inputSDRHoldingToneWhite"),
        recipe.holding_tone_white,
    );
    num(ns_string!("inputMaxHeadroom"), recipe.max_headroom);
    num(
        ns_string!("inputSDRGradientDistance0"),
        recipe.gradient_distance0,
    );
    num(
        ns_string!("inputSDRGradientDistance1"),
        recipe.gradient_distance1,
    );

    Some(filter)
}

static PRIVATE_API: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("NATIVE_GPUI_NO_PRIVATE_GLASS").is_none());

fn private_class(name: &CStr) -> Option<&'static AnyClass> {
    PRIVATE_API.then(|| AnyClass::get(name)).flatten()
}

fn private_instance(name: &CStr) -> Option<Retained<AnyObject>> {
    let class = private_class(name)?;
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![allocated, init]
    }
}

fn private_layer(name: &CStr) -> Option<Retained<CALayer>> {
    private_instance(name).map(|object| unsafe { Retained::cast_unchecked(object) })
}

fn set_number(object: &impl Message, key: &NSString, value: f32) {
    let value = NSNumber::new_f32(value);
    unsafe { msg_send![object, setValue: &*value, forKey: key] }
}

fn set_bool(object: &impl Message, key: &NSString, value: bool) {
    let value = NSNumber::new_bool(value);
    unsafe { msg_send![object, setValue: &*value, forKey: key] }
}

fn set_size(object: &impl Message, key: &NSString, value: Point<f32>) {
    let value = unsafe { NSValue::valueWithSize(NSSize::new(value.x as f64, value.y as f64)) };
    unsafe { msg_send![object, setValue: &*value, forKey: key] }
}

fn set_object(object: &impl Message, key: &NSString, value: Option<&impl Message>) {
    unsafe { msg_send![object, setValue: value, forKey: key] }
}

fn set_fill(object: &impl Message, key: &NSString, color: Option<Hsla>) {
    let Some(color) = color else {
        return set_object(object, key, None::<&AnyObject>);
    };
    let owner = to_nscolor(color);
    let cg: *mut AnyObject = unsafe { msg_send![&*owner, CGColor] };
    set_object(object, key, unsafe { cg.as_ref() });
}

fn without_animation(body: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    body();
    CATransaction::commit();
}
