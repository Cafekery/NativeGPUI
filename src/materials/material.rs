//! `NSVisualEffectView` — the vibrancy materials macOS draws its own chrome with.
//!
//! Materials default to [`Layer::Below`], a sibling *underneath* GPUI's Metal view, so GPUI text
//! composites on top of them. That needs a non-opaque window: see [`crate::window_options`].
//!
//! [`Material`] is the supported surface; [`RawMaterial`] is the whole catalogue, whose
//! undocumented entries and the private knob need the `private-api` feature.

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
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSEdgeInsets, NSRect, NSSize};

#[cfg(feature = "private-api")]
use super::private_api;
use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{Layer, NativeHost, element_key};

/// Which macOS surface this is, named for the role rather than transcribing AppKit's list.
/// Reach for [`RawMaterial`] only when none of these is the thing you mean.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Material {
    /// The whole window's background. Maps to `UnderWindowBackground`; AppKit's same-named
    /// `WindowBackground` is opaque.
    WindowBackground,
    /// A source-list or navigation sidebar. macOS 26 draws it identically to
    /// [`Material::WindowBackground`].
    Sidebar,
    /// The window's titlebar band, for an app drawing its own title area.
    Titlebar,
    /// A header or footer band inside the content: a table header, an inspector's title row.
    Header,
    /// A popover or floating panel attached to a control. Small surfaces only — it transmits
    /// twice what the chrome materials do, which reads as a tint over a large one.
    Popover,
    /// A menu. Small surfaces only, for the same reason as [`Material::Popover`].
    Menu,
    /// A heads-up display: a transport bar, a volume overlay, an inspector over media.
    Hud,
    /// The highlight behind a selected row or cell. [`NativeMaterial::emphasized`] is what turns
    /// its accent tint on; without it a focused row renders grey.
    Selection,
    /// An elevated content surface: a settings card, a grouped box. Opaque, as are `Sheet` and
    /// `UnderPageBackground`; if you wanted blur you wanted [`Material::Popover`].
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

/// Every material `NSVisualEffectView` draws, deprecated ones included, for a browser or for the
/// cases [`Material`]'s nine roles do not cover. The undocumented entries are gated behind
/// `private-api`: they render today and an OS update is free to change them.
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

    /// Empty without `private-api`, so a browser can iterate it either way.
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

/// What the material samples to build its blur.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Blending {
    /// Everything behind the *window*: the desktop, other apps, other windows.
    #[default]
    BehindWindow,
    /// What is drawn behind this view *inside* the window — on [`Layer::Above`], GPUI's own
    /// rendered content. Costs several times [`Blending::BehindWindow`] and is billed in the
    /// WindowServer, so a CPU-only measurement will call it free. A no-op on [`Layer::Below`]:
    /// nothing in the window is beneath it, so AppKit falls back to the window's backdrop.
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

/// A real macOS vibrancy surface: [`Styled`] and [`ParentElement`] like `div()`, with no size of
/// its own.
///
/// ```ignore
/// material("sidebar", Material::Sidebar).w(px(180.)).h_full().p_3().child("Library")
/// material("tip", RawMaterial::ToolTip).w(px(120.)).h(px(28.))
/// ```
///
/// Children are GPUI content painted into the Metal view, so the default [`Layer::Below`] puts
/// them crisply *over* the material. They do not combine with
/// [`NativeMaterial::layer`]`(Layer::Above)`, which covers every GPUI pixel in its rect.
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

    /// AppKit's "first responder is on this surface" look; turns the accent tint on for
    /// [`Material::Selection`].
    pub fn emphasized(mut self, emphasized: bool) -> Self {
        self.emphasized = emphasized;
        self
    }

    /// Round the material's corners, applied as the view's `maskImage`. `layer.cornerRadius`
    /// also clips a behind-window backdrop, but it measures a softer arc and clips subviews too.
    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Where the material sits relative to GPUI's content. Defaults to [`Layer::Below`], which
    /// is what lets GPUI text and icons composite on top of it.
    pub fn layer(mut self, layer: Layer) -> Self {
        self.layer = layer;
        self
    }

    /// Let the material flatten when the window stops being key, AppKit's default. Off here: a
    /// non-key `NSVisualEffectView` collapses to a flat fill, so a sidebar would go grey the
    /// moment you click another window. Turn it on for something whose job is to signal focus.
    pub fn dims_when_inactive(mut self, dims: bool) -> Self {
        self.dims_when_inactive = dims;
        self
    }

    /// Flatten the material to an opaque dark fill, backdrop and vibrancy gone. The one private
    /// `NSVisualEffectView` knob that draws; a macOS without it leaves the material alone.
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
        // `fittingSize` is 0x0 today; asked per axis so a future macOS with an opinion wins.
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
        (window.request_layout(style, children, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
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
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let mask = window.content_mask().bounds;
        let host = NativeHost::get(window, cx);

        let view = host.place(key, self.layer, bounds, mask, |mtm, _| {
            NSVisualEffectView::new(mtm)
        });

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

        for child in &mut self.children {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for NativeMaterial {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Reads the applied radius back off the view's own mask, safe only because `maskImage`
/// round-trips faithfully; a property AppKit normalises would be rewritten every frame.
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

/// A nine-part resizable rounded-rect mask, the smallest that can express `radius`.
///
/// `drawingHandler:` rather than `lockFocus` because the handler is re-run at whatever scale
/// AppKit needs, so the arc rasterises at backing scale instead of stair-stepping from 1x. Cap
/// insets pin the corners so only the middle point stretches, keeping the radius constant.
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
            // Only alpha is read; the clear start is what leaves the corners transparent.
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
