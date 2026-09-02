//! `NSVisualEffectView`: the vibrancy materials macOS draws its own chrome with.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ParentElement, Pixels, Refineable, Style, StyleRefinement, Styled, Window, px,
};
use objc2::rc::Retained;
use objc2::runtime::Bool;
#[cfg(feature = "private-api")]
use objc2::sel;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSResponder, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{MainThreadMarker, NSEdgeInsets, NSObject, NSPoint, NSRect, NSSize};

#[cfg(feature = "private-api")]
use super::private_api;
use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{Layer, NativeHost, element_key};

/// The supported materials, named for their role.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Material {
    /// Maps to `UnderWindowBackground`; AppKit's `WindowBackground` is opaque.
    WindowBackground,
    Sidebar,
    Titlebar,
    Header,
    /// Small surfaces only: it transmits twice what the chrome materials do.
    Popover,
    Menu,
    Hud,
    /// The accent tint needs [`NativeMaterial::emphasized`].
    Selection,
    /// Opaque.
    Card,
}

impl From<Material> for RawMaterial {
    fn from(material: Material) -> Self {
        match material {
            Material::WindowBackground => Self::UnderWindowBackground,
            Material::Sidebar => Self::Sidebar,
            Material::Titlebar => Self::Titlebar,
            Material::Header => Self::HeaderView,
            Material::Popover => Self::Popover,
            Material::Menu => Self::Menu,
            Material::Hud => Self::HudWindow,
            Material::Selection => Self::Selection,
            Material::Card => Self::ContentBackground,
        }
    }
}

/// Every material `NSVisualEffectView` draws; the undocumented ones need `private-api`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum RawMaterial {
    AppearanceBased,
    Light,
    Dark,
    Titlebar,
    Selection,
    Menu,
    Popover,
    Sidebar,
    MediumLight,
    UltraDark,
    HeaderView,
    Sheet,
    WindowBackground,
    HudWindow,
    FullScreenUi,
    ToolTip,
    ContentBackground,
    UnderWindowBackground,
    UnderPageBackground,
    #[cfg(feature = "private-api")]
    Undocumented14,
    #[cfg(feature = "private-api")]
    Undocumented20,
    #[cfg(feature = "private-api")]
    Undocumented29,
    #[cfg(feature = "private-api")]
    Undocumented30,
    #[cfg(feature = "private-api")]
    Undocumented32,
    #[cfg(feature = "private-api")]
    Undocumented35,
    #[cfg(feature = "private-api")]
    Undocumented36,
    #[cfg(feature = "private-api")]
    Undocumented37,
    #[cfg(feature = "private-api")]
    Undocumented38,
}

impl RawMaterial {
    pub const DOCUMENTED: &'static [Self] = &[
        Self::Titlebar,
        Self::WindowBackground,
        Self::UnderWindowBackground,
        Self::HeaderView,
        Self::Menu,
        Self::Popover,
        Self::ToolTip,
        Self::Selection,
        Self::Sidebar,
        Self::ContentBackground,
        Self::UnderPageBackground,
        Self::Sheet,
        Self::HudWindow,
        Self::FullScreenUi,
        Self::AppearanceBased,
        Self::Light,
        Self::Dark,
        Self::MediumLight,
        Self::UltraDark,
    ];

    #[cfg(feature = "private-api")]
    pub const UNDOCUMENTED: &'static [Self] = &[
        Self::Undocumented14,
        Self::Undocumented20,
        Self::Undocumented29,
        Self::Undocumented30,
        Self::Undocumented32,
        Self::Undocumented35,
        Self::Undocumented36,
        Self::Undocumented37,
        Self::Undocumented38,
    ];
    #[cfg(not(feature = "private-api"))]
    pub const UNDOCUMENTED: &'static [Self] = &[];

    pub fn all() -> impl Iterator<Item = Self> {
        Self::DOCUMENTED.iter().chain(Self::UNDOCUMENTED).copied()
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Titlebar => "Titlebar",
            Self::WindowBackground => "Window Background",
            Self::UnderWindowBackground => "Under Window Background",
            Self::HeaderView => "Header View",
            Self::Menu => "Menu",
            Self::Popover => "Popover",
            Self::ToolTip => "Tool Tip",
            Self::Selection => "Selection",
            Self::Sidebar => "Sidebar",
            Self::ContentBackground => "Content Background",
            Self::UnderPageBackground => "Under Page Background",
            Self::Sheet => "Sheet",
            Self::HudWindow => "HUD Window",
            Self::FullScreenUi => "Full Screen UI",
            Self::AppearanceBased => "Appearance Based",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::MediumLight => "Medium Light",
            Self::UltraDark => "Ultra Dark",
            #[cfg(feature = "private-api")]
            Self::Undocumented14 => "Material 14",
            #[cfg(feature = "private-api")]
            Self::Undocumented20 => "Material 20",
            #[cfg(feature = "private-api")]
            Self::Undocumented29 => "Material 29",
            #[cfg(feature = "private-api")]
            Self::Undocumented30 => "Material 30",
            #[cfg(feature = "private-api")]
            Self::Undocumented32 => "Material 32",
            #[cfg(feature = "private-api")]
            Self::Undocumented35 => "Material 35",
            #[cfg(feature = "private-api")]
            Self::Undocumented36 => "Material 36",
            #[cfg(feature = "private-api")]
            Self::Undocumented37 => "Material 37",
            #[cfg(feature = "private-api")]
            Self::Undocumented38 => "Material 38",
        }
    }

    pub fn is_deprecated(self) -> bool {
        matches!(
            self,
            Self::AppearanceBased | Self::Light | Self::Dark | Self::MediumLight | Self::UltraDark
        )
    }

    #[allow(deprecated)]
    pub(crate) fn appkit(self) -> NSVisualEffectMaterial {
        match self {
            Self::Titlebar => NSVisualEffectMaterial::Titlebar,
            Self::WindowBackground => NSVisualEffectMaterial::WindowBackground,
            Self::UnderWindowBackground => NSVisualEffectMaterial::UnderWindowBackground,
            Self::HeaderView => NSVisualEffectMaterial::HeaderView,
            Self::Menu => NSVisualEffectMaterial::Menu,
            Self::Popover => NSVisualEffectMaterial::Popover,
            Self::ToolTip => NSVisualEffectMaterial::ToolTip,
            Self::Selection => NSVisualEffectMaterial::Selection,
            Self::Sidebar => NSVisualEffectMaterial::Sidebar,
            Self::ContentBackground => NSVisualEffectMaterial::ContentBackground,
            Self::UnderPageBackground => NSVisualEffectMaterial::UnderPageBackground,
            Self::Sheet => NSVisualEffectMaterial::Sheet,
            Self::HudWindow => NSVisualEffectMaterial::HUDWindow,
            Self::FullScreenUi => NSVisualEffectMaterial::FullScreenUI,
            Self::AppearanceBased => NSVisualEffectMaterial::AppearanceBased,
            Self::Light => NSVisualEffectMaterial::Light,
            Self::Dark => NSVisualEffectMaterial::Dark,
            Self::MediumLight => NSVisualEffectMaterial::MediumLight,
            Self::UltraDark => NSVisualEffectMaterial::UltraDark,
            #[cfg(feature = "private-api")]
            Self::Undocumented14 => NSVisualEffectMaterial(14),
            #[cfg(feature = "private-api")]
            Self::Undocumented20 => NSVisualEffectMaterial(20),
            #[cfg(feature = "private-api")]
            Self::Undocumented29 => NSVisualEffectMaterial(29),
            #[cfg(feature = "private-api")]
            Self::Undocumented30 => NSVisualEffectMaterial(30),
            #[cfg(feature = "private-api")]
            Self::Undocumented32 => NSVisualEffectMaterial(32),
            #[cfg(feature = "private-api")]
            Self::Undocumented35 => NSVisualEffectMaterial(35),
            #[cfg(feature = "private-api")]
            Self::Undocumented36 => NSVisualEffectMaterial(36),
            #[cfg(feature = "private-api")]
            Self::Undocumented37 => NSVisualEffectMaterial(37),
            #[cfg(feature = "private-api")]
            Self::Undocumented38 => NSVisualEffectMaterial(38),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Blending {
    /// Everything behind the window.
    #[default]
    BehindWindow,
    /// What is drawn behind the view inside the window; a no-op on `Layer::Below`.
    WithinWindow,
}

impl Blending {
    pub(crate) fn appkit(self) -> NSVisualEffectBlendingMode {
        match self {
            Self::BehindWindow => NSVisualEffectBlendingMode::BehindWindow,
            Self::WithinWindow => NSVisualEffectBlendingMode::WithinWindow,
        }
    }
}

/// A vibrancy surface that is `Styled` and a `ParentElement` like `div()`. Children are GPUI
/// content, so the default `Layer::Below` puts them crisply over the material.
pub fn material(id: impl Into<ElementId>, material: impl Into<RawMaterial>) -> NativeMaterial {
    NativeMaterial {
        id: id.into(),
        material: material.into(),
        blending: Blending::BehindWindow,
        emphasized: false,
        corner_radius: px(0.),
        layer: Layer::Below,
        dims_when_inactive: false,
        #[cfg(feature = "private-api")]
        artificial_chameleon: false,
        style: StyleRefinement::default(),
        children: Vec::new(),
    }
}

pub struct NativeMaterial {
    id: ElementId,
    material: RawMaterial,
    blending: Blending,
    emphasized: bool,
    corner_radius: Pixels,
    layer: Layer,
    dims_when_inactive: bool,
    #[cfg(feature = "private-api")]
    artificial_chameleon: bool,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl NativeMaterial {
    pub fn blending(mut self, blending: Blending) -> Self {
        self.blending = blending;
        self
    }

    pub fn emphasized(mut self, emphasized: bool) -> Self {
        self.emphasized = emphasized;
        self
    }

    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = radius;
        self
    }

    pub fn layer(mut self, layer: Layer) -> Self {
        self.layer = layer;
        self
    }

    /// Off by default: a non-key `NSVisualEffectView` collapses to a flat fill.
    pub fn dims_when_inactive(mut self, dims: bool) -> Self {
        self.dims_when_inactive = dims;
        self
    }

    /// Flatten the material to an opaque dark fill.
    #[cfg(feature = "private-api")]
    pub fn artificial_chameleon(mut self, forced: bool) -> Self {
        self.artificial_chameleon = forced;
        self
    }
}

impl Styled for NativeMaterial {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for NativeMaterial {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Element for NativeMaterial {
    type RequestLayoutState = Style;
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
    ) -> (LayoutId, Style) {
        let fitting = fitting_size("NSVisualEffectView", |mtm| {
            NSVisualEffectView::new(mtm).fittingSize()
        });
        let mut style = Style::default();
        if fitting.width > 0. {
            style.size.width = px(fitting.width as f32).into();
        }
        if fitting.height > 0. {
            style.size.height = px(fitting.height as f32).into();
        }
        style.refine(&self.style);
        let children = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect::<Vec<_>>();
        (window.request_layout(style.clone(), children, cx), style)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Style,
        window: &mut Window,
        cx: &mut App,
    ) {
        for child in &mut self.children {
            child.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        style: &mut Style,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let mask = window.content_mask().bounds;
        let host = NativeHost::get(window, cx);
        let view = host.place(key, self.layer, bounds, mask, |mtm, _| Backdrop::new(mtm));

        let material = self.material.appkit();
        if view.material() != material {
            view.setMaterial(material);
        }
        let blending = self.blending.appkit();
        if view.blendingMode() != blending {
            view.setBlendingMode(blending);
        }
        let state = if self.dims_when_inactive {
            NSVisualEffectState::FollowsWindowActiveState
        } else {
            NSVisualEffectState::Active
        };
        if view.state() != state {
            view.setState(state);
        }
        if view.isEmphasized() != self.emphasized {
            view.setEmphasized(self.emphasized);
        }
        #[cfg(feature = "private-api")]
        private_api::send_bool(
            &*view,
            sel!(_setForcesArtificialChameleon:),
            self.artificial_chameleon,
        );
        apply_corner_radius(&view, self.corner_radius.to_f64());

        style.paint(bounds, window, cx, |window, cx| {
            for child in &mut self.children {
                child.paint(window, cx);
            }
        });
    }
}

impl IntoElement for NativeMaterial {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

define_class!(
    /// A material that declines every mouse point: a stock one claims them, and at `Layer::Above`
    /// that eats every click under it.
    #[unsafe(super(NSVisualEffectView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGBackdrop"]
    struct Backdrop;

    impl Backdrop {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl Backdrop {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

fn apply_corner_radius(view: &NSVisualEffectView, radius: f64) {
    let applied = view
        .maskImage()
        .map_or(0., |mask| (mask.size().width - 1.) / 2.);
    if applied == radius {
        return;
    }
    if radius <= 0. {
        view.setMaskImage(None);
    } else {
        view.setMaskImage(Some(&corner_mask(radius)));
    }
}

/// A nine-part resizable rounded-rect mask; the drawing handler re-runs at backing scale.
fn corner_mask(radius: f64) -> Retained<NSImage> {
    thread_local! {
        static CACHE: RefCell<HashMap<u64, Retained<NSImage>>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        if let Some(mask) = cache.borrow().get(&radius.to_bits()) {
            return mask.clone();
        }
        let side = radius * 2. + 1.;
        let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
            let path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
            NSColor::blackColor().setFill();
            path.fill();
            Bool::YES
        });
        let mask =
            NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(side, side), false, &draw);
        mask.setCapInsets(NSEdgeInsets {
            top: radius,
            left: radius,
            bottom: radius,
            right: radius,
        });
        cache.borrow_mut().insert(radius.to_bits(), mask.clone());
        mask
    })
}
