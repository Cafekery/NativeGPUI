//! macOS 26 liquid glass: `NSGlassEffectView`, batched by `NSGlassEffectContainerView`.
//!
//! Ordering decides which side of the glass GPUI lands on. [`crate::Layer::Above`] puts the
//! panel over GPUI's Metal view, so GPUI content is frosted by it; [`crate::Layer::Below`] puts
//! it under, so GPUI composites over it and a GPUI label on glass stays sharp.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, Display, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Rgba, Size, Style, Window, point, px,
    relative,
};
use objc2::rc::Retained;
use objc2_app_kit::{
    NSColor, NSGlassEffectContainerView, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

#[cfg(feature = "private-api")]
use super::glass_private::{GlassInteractionState, GlassSubvariant, GlassVariant, Private};
use crate::bridge::appkit::{Container, rects_equal};
use crate::bridge::host::{Layer, NativeHost, element_key};
use crate::system::color::to_nscolor;

// No `hitTest:` override, and the obvious one cannot work: `NSGlassEffectView` builds private
// subviews, so `super`'s `hitTest:` returns one of *those* and [`Container`]'s "null if the hit
// is me" shape never fires. Unneeded anyway: AppKit walks unhandled mouse events up to GPUI.

/// [`to_nscolor`] memoized: the tint diff runs every frame, and tints come from a palette.
fn tint_color(color: Hsla) -> Retained<NSColor> {
    thread_local! {
        static CACHE: RefCell<HashMap<u32, Retained<NSColor>>> = RefCell::new(HashMap::new());
    }
    let rgba = Rgba::from(color);
    let byte = |c: f32| (c.clamp(0., 1.) * 255.).round() as u32;
    let key = byte(rgba.r) << 24 | byte(rgba.g) << 16 | byte(rgba.b) << 8 | byte(rgba.a);
    CACHE.with(|cache| {
        if let Some(color) = cache.borrow().get(&key) {
            return color.clone();
        }
        let color = to_nscolor(color);
        cache.borrow_mut().insert(key, color.clone());
        color
    })
}

/// The `NSGlassEffectContainerView`s whose GPUI children are painting right now, innermost last.
///
/// GPUI paints depth-first on the main thread in one synchronous pass, so a stack pushed by a
/// container's `paint` and truncated after it is exactly scoped to that subtree — which is how
/// a [`Glass`] finds its container without either element knowing about the other.
struct OpenContainer {
    content: Retained<NSView>,
    layer: Layer,
}

thread_local! {
    static OPEN: RefCell<Vec<OpenContainer>> = const { RefCell::new(Vec::new()) };
}

/// A panel of macOS 26 liquid glass, frosting the GPUI content painted beneath it. It fills the
/// box you give it (`div().w(px(220.)).h(px(120.)).child(glass("card"))`) and draws nothing
/// itself — every pixel is AppKit's.
///
/// Glass samples whatever is beneath it, so over an unpainted region of an opaque window it has
/// nothing to sample and collapses to a flat fill. A flat GPUI background is enough; genuinely
/// nothing is not.
///
/// Everything GPUI paints in this rect is frosted, text included; for a crisp GPUI label *on*
/// glass use [`Glass::frosting_desktop`]. Mouse events pass through to GPUI and to any hosted
/// control ordered above the panel.
pub fn glass(id: impl Into<ElementId>) -> Glass {
    Glass {
        id: id.into(),
        layer: Layer::Above,
        corner_radius: None,
        tint: None,
        clear: false,
        #[cfg(feature = "private-api")]
        private: Private::default(),
    }
}

pub struct Glass {
    id: ElementId,
    layer: Layer,
    corner_radius: Option<Pixels>,
    tint: Option<Hsla>,
    clear: bool,
    #[cfg(feature = "private-api")]
    private: Private,
}

impl Glass {
    /// Frost the GPUI content painted beneath this panel. The default, stated at the call site.
    pub fn frosting_gpui(mut self) -> Self {
        self.layer = Layer::Above;
        self
    }

    /// Frost the desktop behind the window instead, putting the panel *under* GPUI's Metal view
    /// so GPUI paints crisply on top of it. Needs a non-opaque window
    /// ([`crate::window_options`]) and a rect your scene leaves transparent — an opaque `bg()`
    /// anywhere in it hides the glass. A panel here can never receive input.
    pub fn frosting_desktop(mut self) -> Self {
        self.layer = Layer::Below;
        self
    }

    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = Some(radius);
        self
    }

    /// Tint the glass and its resolved backdrop toward a colour. Alpha is the strength: at 1.0
    /// no frost is left, so start around 0.3. macOS drops the chroma entirely while the window
    /// is not key, leaving only the luminance shift, and no property pins that.
    pub fn tint(mut self, color: impl Into<Hsla>) -> Self {
        self.tint = Some(color.into());
        self
    }

    /// `NSGlassEffectViewStyle::Clear`, which tints rather than blurs — most of the backdrop's
    /// structure survives it. For imagery you want to stay legible, never for a lighter frost.
    pub fn clear(mut self, clear: bool) -> Self {
        self.clear = clear;
        self
    }
}

#[cfg(feature = "private-api")]
impl Glass {
    /// Draw an internal recipe instead of letting `style` pick one. Overrides [`Glass::clear`].
    pub fn variant(mut self, variant: GlassVariant) -> Self {
        self.private.variant = Some(variant);
        self
    }

    /// Refine the variant. Nothing is sent unless [`GlassVariant::accepts`] the pairing, so ask
    /// before offering it as a control.
    pub fn subvariant(mut self, subvariant: GlassSubvariant) -> Self {
        self.private.subvariant = subvariant;
        self
    }

    pub fn interaction_state(mut self, state: GlassInteractionState) -> Self {
        self.private.interaction = state;
        self
    }

    /// Recede the glass so something in front of it reads more clearly.
    pub fn subdued(mut self, subdued: bool) -> Self {
        self.private.subdued = subdued;
        self
    }

    /// Lay a darkening scrim into the glass.
    pub fn scrim(mut self, scrim: bool) -> Self {
        self.private.scrim = scrim;
        self
    }

    /// Refract this panel's own content rather than its backdrop.
    pub fn content_lensing(mut self, lensing: bool) -> Self {
        self.private.lensing = lensing;
        self
    }
}

pub(crate) fn glass_style(clear: bool) -> NSGlassEffectViewStyle {
    if clear {
        NSGlassEffectViewStyle::Clear
    } else {
        NSGlassEffectViewStyle::Regular
    }
}

impl Element for Glass {
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
        let style = Style {
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Default::default()
        };
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

        // A member takes its container's ordering; across GPUI's content it could not batch.
        let container = OPEN.with(|open| {
            open.borrow()
                .last()
                .map(|open| (open.content.clone(), open.layer))
        });
        let layer = container.as_ref().map_or(self.layer, |&(_, layer)| layer);
        let parent = container.as_ref().map(|(content, _)| &**content);

        // Parentage is set at creation only: `commit` re-adds every slot to its layer container
        // whenever placement order moves, so a later reparent is silently undone.
        let view = host.place_in(key, layer, parent, bounds, mask, |mtm, _| {
            NSGlassEffectView::new(mtm)
        });

        #[cfg(not(feature = "private-api"))]
        {
            let style = glass_style(self.clear);
            if view.style() != style {
                view.setStyle(style);
            }
        }
        #[cfg(feature = "private-api")]
        self.private.apply(&view, self.clear);
        if let Some(radius) = self.corner_radius {
            let radius = radius.to_f64();
            if view.cornerRadius() != radius {
                view.setCornerRadius(radius);
            }
        }
        let tint = self.tint.map(tint_color);
        if view.tintColor().as_deref() != tint.as_deref() {
            view.setTintColor(tint.as_deref());
        }
    }
}

impl IntoElement for Glass {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Batches the [`glass`] panels in its subtree into one glass render pass. Wrap any screen with
/// more than a handful of them.
///
/// Panels find their container automatically and inherit its ordering. Being inside the
/// container's `contentView` is what does the batching — a container view merely present in the
/// hierarchy buys nothing.
///
/// The container draws nothing and spans the whole window, so a member's frame means the same
/// thing inside it as outside. For layout it behaves as `div().size_full().flex()`; put an
/// ordinary `div` inside for gaps and direction.
pub fn glass_container(id: impl Into<ElementId>) -> GlassContainer {
    GlassContainer {
        id: id.into(),
        layer: Layer::Above,
        spacing: None,
        children: Vec::new(),
    }
}

pub struct GlassContainer {
    id: ElementId,
    layer: Layer,
    spacing: Option<Pixels>,
    children: Vec<AnyElement>,
}

impl GlassContainer {
    /// Members frost the GPUI content painted beneath them. The default; see [`glass`].
    pub fn frosting_gpui(mut self) -> Self {
        self.layer = Layer::Above;
        self
    }

    /// Members frost the desktop and GPUI paints crisply over them. The requirements on
    /// [`Glass::frosting_desktop`] apply to every member.
    pub fn frosting_desktop(mut self) -> Self {
        self.layer = Layer::Below;
        self
    }

    /// How close two members must be before macOS merges them into one blob of glass. Zero, the
    /// default, batches without merging.
    pub fn spacing(mut self, spacing: Pixels) -> Self {
        self.spacing = Some(spacing);
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }
}

impl Element for GlassContainer {
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
        let children: Vec<LayoutId> = self
            .children
            .iter_mut()
            .map(|child| child.request_layout(window, cx))
            .collect();
        let style = Style {
            display: Display::Flex,
            size: Size {
                width: relative(1.).into(),
                height: relative(1.).into(),
            },
            ..Default::default()
        };
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
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let key = element_key(id);
        let host = NativeHost::get(window, cx);
        let viewport = Bounds {
            origin: point(px(0.), px(0.)),
            size: window.viewport_size(),
        };

        let view = host.place(key, self.layer, viewport, viewport, |mtm, _| {
            let view = NSGlassEffectContainerView::new(mtm);
            // Flipped, so a member's frame is GPUI's coordinate space verbatim.
            let content = Container::new(mtm, NSRect::ZERO, false);
            view.setContentView(Some(content.as_ref()));
            view
        });
        // Nullable, and re-read from a pooled view whose `setContentView:` ran only on creation.
        // Members already degrade to standalone panels, so never panic in a consumer's paint.
        let content = view.contentView();
        if let Some(content) = &content {
            let fill = NSRect::new(
                NSPoint::ZERO,
                NSSize::new(viewport.size.width.to_f64(), viewport.size.height.to_f64()),
            );
            if !rects_equal(content.frame(), fill) {
                content.setFrame(fill);
            }
        }
        if let Some(spacing) = self.spacing {
            let spacing = spacing.to_f64();
            if view.spacing() != spacing {
                view.setSpacing(spacing);
            }
        }

        // Truncate rather than pop: a child that failed to pop cannot leak into a sibling.
        let depth = OPEN.with(|open| {
            let mut open = open.borrow_mut();
            let depth = open.len();
            if let Some(content) = content {
                open.push(OpenContainer {
                    content,
                    layer: self.layer,
                });
            }
            depth
        });
        for child in &mut self.children {
            child.paint(window, cx);
        }
        OPEN.with(|open| open.borrow_mut().truncate(depth));
    }
}

impl IntoElement for GlassContainer {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
