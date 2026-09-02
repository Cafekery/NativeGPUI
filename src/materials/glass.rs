//! macOS 26 liquid glass: `NSGlassEffectView`, batched by `NSGlassEffectContainerView`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::Location;
use std::ptr;

use gpui::{
    AnyElement, App, Bounds, Display, Element, ElementId, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Pixels, Rgba, Size, Style, Window, point, px,
    relative,
};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSGlassEffectContainerView, NSGlassEffectView, NSGlassEffectViewStyle, NSResponder,
    NSView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

#[cfg(feature = "private-api")]
use super::glass_private::{GlassInteractionState, GlassSubvariant, GlassVariant, Private};
use crate::bridge::appkit::{Container, rects_equal};
use crate::bridge::host::{Layer, NativeHost, Slot, element_key, hosted_element};
use crate::system::color::to_nscolor;

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

/// The containers whose children are painting right now, innermost last; a panel joins the
/// innermost one.
struct OpenContainer {
    content: Retained<NSView>,
    layer: Layer,
}

thread_local! {
    static OPEN: RefCell<Vec<OpenContainer>> = const { RefCell::new(Vec::new()) };
}

/// A panel of liquid glass filling the box it is given. Over an unpainted region of an opaque
/// window it has nothing to sample and collapses to a flat fill.
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
    /// Frost the GPUI content painted beneath the panel (the default).
    pub fn frosting_gpui(mut self) -> Self {
        self.layer = Layer::Above;
        self
    }

    /// Frost the desktop instead, under GPUI's view: needs a non-opaque window and a rect the
    /// scene leaves transparent, and can never receive input.
    pub fn frosting_desktop(mut self) -> Self {
        self.layer = Layer::Below;
        self
    }

    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = Some(radius);
        self
    }

    /// Alpha is the strength; macOS drops the chroma while the window is not key.
    pub fn tint(mut self, color: impl Into<Hsla>) -> Self {
        self.tint = Some(color.into());
        self
    }

    /// `NSGlassEffectViewStyle::Clear`: tints rather than blurs.
    pub fn clear(mut self, clear: bool) -> Self {
        self.clear = clear;
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
        let container = OPEN.with(|open| {
            open.borrow()
                .last()
                .map(|open| (open.content.clone(), open.layer))
        });
        let layer = container.as_ref().map_or(self.layer, |&(_, layer)| layer);
        let parent = container.as_ref().map(|(content, _)| &**content);
        let view = slot
            .host
            .place_in(slot.key, layer, parent, slot.bounds, slot.mask, |mtm, _| {
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

hosted_element!(Glass);

#[cfg(feature = "private-api")]
impl Glass {
    /// Overrides [`Glass::clear`].
    pub fn variant(mut self, variant: GlassVariant) -> Self {
        self.private.variant = Some(variant);
        self
    }

    /// Sent only where [`GlassVariant::accepts`] the pairing.
    pub fn subvariant(mut self, subvariant: GlassSubvariant) -> Self {
        self.private.subvariant = subvariant;
        self
    }

    pub fn interaction_state(mut self, state: GlassInteractionState) -> Self {
        self.private.interaction = state;
        self
    }

    pub fn subdued(mut self, subdued: bool) -> Self {
        self.private.subdued = subdued;
        self
    }

    pub fn scrim(mut self, scrim: bool) -> Self {
        self.private.scrim = scrim;
        self
    }

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

define_class!(
    /// `NSGlassEffectContainerView` that claims a click only where a member is: stock, it answers
    /// `hitTest:` with itself across its whole bounds.
    #[unsafe(super(NSGlassEffectContainerView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGGlassContainer"]
    struct Batch;

    impl Batch {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let Some(content) = self.contentView() else {
                return ptr::null_mut();
            };
            let local = match unsafe { self.superview() } {
                Some(parent) => self.convertPoint_fromView(point, Some(&parent)),
                None => point,
            };
            unsafe { msg_send![&*content, hitTest: local] }
        }
    }
);

impl Batch {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

/// Batches the [`glass`] panels in its subtree into one render pass. Spans the whole window and
/// lays out like `div().size_full().flex()`.
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
    pub fn frosting_gpui(mut self) -> Self {
        self.layer = Layer::Above;
        self
    }

    pub fn frosting_desktop(mut self) -> Self {
        self.layer = Layer::Below;
        self
    }

    /// How close two members must be before macOS merges them into one blob.
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
            let view = Batch::new(mtm);
            let content = Container::new(mtm, NSRect::ZERO, false);
            view.setContentView(Some(content.as_ref()));
            view
        });
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
