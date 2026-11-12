use std::cell::{Cell, RefCell};
use std::f32::consts::TAU;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Div, FontWeight, Stateful, Window, canvas, div, prelude::*, px};
use native_gpui::{
    Layer, NativeHandler, NativeHost, ValueKind, native_label, native_opacity, wire,
};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityDisclosureTriangleRole, NSBezierPath, NSButton, NSColor,
    NSControl, NSEvent, NSFontWeightBold, NSImage, NSImageScaling, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSImageView, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::form::{Form, INSET, MARGIN, SECTION, key_of};

const RESPONSE: f32 = 0.25;
const SETTLED: Duration = Duration::from_millis(380);
const CHEVRON: f64 = 11.2;
const BOX: f32 = 16.;
const REACH: f32 = 4.;
const RING: f64 = 6.;

pub(crate) struct Fold {
    open: bool,
    from: f32,
    moved: Option<Instant>,
    heights: Rc<RefCell<Vec<f32>>>,
}

impl Fold {
    pub(crate) fn new(open: bool) -> Self {
        Fold {
            open,
            from: f32::from(u8::from(open)),
            moved: None,
            heights: Rc::default(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn toggle(&mut self) {
        self.from = self.state().0;
        self.open = !self.open;
        self.moved = Some(Instant::now());
    }

    pub(crate) fn moving(&self) -> bool {
        self.state().1
    }

    fn state(&self) -> (f32, bool) {
        let to = f32::from(u8::from(self.open));
        let since = self.moved.map(|moved| moved.elapsed());
        match since.filter(|since| *since < SETTLED) {
            Some(since) => (
                self.from + (to - self.from) * spring(since.as_secs_f32()),
                true,
            ),
            None => (to, false),
        }
    }
}

fn spring(seconds: f32) -> f32 {
    let at = TAU / RESPONSE * seconds;
    1. - (1. + at) * (-at).exp()
}

pub(crate) enum Part {
    Card(&'static str, Vec<AnyElement>),
    Plain(AnyElement),
}

pub(crate) fn section(
    form: &Form,
    id: &'static str,
    title: &str,
    fold: &Fold,
    toggle: impl Fn(&mut Window, &mut App) + 'static,
    parts: Vec<(f32, Part)>,
) -> Stateful<Div> {
    let (openness, moving) = fold.state();
    let toggle: NativeHandler = Rc::new(move |_, window, cx| toggle(window, cx));
    div()
        .id(id)
        .pt(px(SECTION))
        .flex()
        .flex_col()
        .child(header(id, title, openness, fold.open, toggle))
        .when(fold.open || moving, |section| {
            section.children(body(form, fold, openness, moving, parts))
        })
}

fn body(
    form: &Form,
    fold: &Fold,
    openness: f32,
    moving: bool,
    parts: Vec<(f32, Part)>,
) -> Vec<AnyElement> {
    let natural = {
        let mut heights = fold.heights.borrow_mut();
        heights.resize(parts.len(), 0.);
        heights.clone()
    };
    let total: f32 = parts.iter().map(|(gap, _)| gap).sum::<f32>() + natural.iter().sum::<f32>();
    let mut edge = openness * total;
    let mut reveal = |size: f32| {
        let shown = size.min(edge);
        edge -= shown;
        shown
    };
    parts
        .into_iter()
        .zip(natural)
        .enumerate()
        .map(|(at, ((gap, part), tall))| {
            let (gap, shown) = (reveal(gap), reveal(tall));
            let heights = fold.heights.clone();
            let measure = canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    if let Some(tall) = heights.borrow_mut().get_mut(at) {
                        *tall = bounds.size.height.into();
                    }
                },
            )
            .absolute()
            .size_full();
            let (clip, held) = match part {
                Part::Card(id, rows) => (form.shell(id), div().children(form.rows(rows))),
                Part::Plain(content) => (div().id(at), div().child(content)),
            };
            let clip = clip
                .mt(px(gap))
                .when(moving, |clip| clip.h(px(shown)).overflow_hidden())
                .child(held.relative().child(measure));
            native_opacity(openness, clip).into_any_element()
        })
        .collect()
}

fn header(id: &str, title: &str, openness: f32, open: bool, toggle: NativeHandler) -> Div {
    let key = key_of(("ng.finder.fold", id));
    let named = NSString::from_str(title);
    div()
        .relative()
        .mx(px(MARGIN))
        .px(px(INSET))
        .flex()
        .items_center()
        .justify_between()
        .child(native_label("title", title).weight(FontWeight::BOLD))
        .child(
            div()
                .flex_none()
                .size(px(BOX))
                .child(chevron(key, openness)),
        )
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let host = NativeHost::get(window, cx);
                    let mask = window.content_mask().bounds;
                    let button = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
                        let button = FoldButton::new(mtm, &named);
                        wire(&button, target, key);
                        button
                    });
                    button.show(open);
                    host.on_change(key, ValueKind::None, Some(toggle.clone()));
                },
            )
            .absolute()
            .left_0()
            .right_0()
            .top(px(-REACH))
            .bottom(px(-REACH)),
        )
}

fn chevron(key: u64, openness: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let host = NativeHost::get(window, cx);
            let mask = window.content_mask().bounds;
            let key = key_of((key, "chevron"));
            let chevron = host.place(key, Layer::Above, bounds, mask, |mtm, _| Chevron::new(mtm));
            chevron.turn(f64::from(openness) * -90.);
        },
    )
    .size_full()
}

define_class!(
    #[unsafe(super(NSButton, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoFold"]
    #[ivars = Cell<Option<bool>>]
    struct FoldButton;

    impl FoldButton {
        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), RING, RING)
                .fill();
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            self.bounds()
        }

        // NSButton's own keyDown: passes Space up the responder chain.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let space = event
                .charactersIgnoringModifiers()
                .is_some_and(|keys| keys.to_string() == " ");
            if space {
                unsafe { self.performClick(None) }
            } else {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }
    }
);

impl FoldButton {
    fn new(mtm: MainThreadMarker, title: &NSString) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(None));
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setTransparent(true);
        this.setTitle(title);
        this.setAccessibilityRole(Some(unsafe { NSAccessibilityDisclosureTriangleRole }));
        this
    }

    fn show(&self, open: bool) {
        if self.ivars().replace(Some(open)) != Some(open) {
            unsafe { self.setAccessibilityValue(Some(&NSNumber::new_bool(open))) };
        }
    }
}

struct ChevronIvars {
    glyph: Retained<NSImageView>,
    angle: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoFoldChevron"]
    #[ivars = ChevronIvars]
    struct Chevron;

    impl Chevron {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl Chevron {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(BOX.into(), BOX.into()));
        let glyph: Retained<NSImageView> =
            unsafe { msg_send![NSImageView::alloc(mtm), initWithFrame: frame] };
        glyph.setImageScaling(NSImageScaling::ScaleNone);
        glyph.setImage(symbol().as_deref());
        glyph.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        let this = Self::alloc(mtm).set_ivars(ChevronIvars {
            glyph: glyph.clone(),
            angle: Cell::new(0.),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.addSubview(&glyph);
        this
    }

    fn turn(&self, angle: f64) {
        let ivars = self.ivars();
        if ivars.angle.get() != angle {
            ivars.angle.set(angle);
            ivars.glyph.setFrameCenterRotation(angle);
        }
    }
}

fn symbol() -> Option<Retained<NSImage>> {
    let name = NSString::from_str("chevron.right");
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        CHEVRON,
        unsafe { NSFontWeightBold },
        NSImageSymbolScale::Small,
    );
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&name, None)?
        .imageWithSymbolConfiguration(&configuration)
}
