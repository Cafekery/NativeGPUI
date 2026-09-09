//! `NSScroller` over GPUI content: the overlay knob, the fade it drives itself, and the legacy
//! style System Settings can pin on.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, ElementId, Style, Window};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSControl, NSControlSize, NSEvent,
    NSPreferredScrollerStyleDidChangeNotification, NSResponder, NSScroller, NSScrollerPart,
    NSScrollerStyle, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSNotificationCenter, NSObject, NSObjectNSDelayedPerforming, NSPoint, NSRect,
    NSSize,
};

use crate::bridge::appkit::fitting_size_keyed;
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHandler, NativeRead, NativeValue, Slot, hosted_element, wire,
};
use crate::controls::fitted;

/// How long an overlay scroller stays up after the last move, and how long it takes to go.
const LINGER: f64 = 1.0;
const FADE: f64 = 0.35;

/// A vertical scroller: `value` is `0.0..=1.0` down the track, `proportion` the visible share of
/// the content. Draw it only while the content overflows.
pub fn native_scroller(id: impl Into<ElementId>, value: f64, proportion: f64) -> NativeScroller {
    NativeScroller {
        id: id.into(),
        value,
        proportion,
        on_change: None,
    }
}

pub struct NativeScroller {
    id: ElementId,
    value: f64,
    proportion: f64,
    on_change: Option<NativeHandler>,
}

/// The value and proportion last written, which a knob drag moves out from under us.
#[derive(Clone, Default)]
struct Held(Rc<Cell<Option<(f64, f64)>>>);

impl NativeScroller {
    /// Fires continuously while the knob is dragged, and once per click in the track.
    pub fn on_change(mut self, handler: impl Fn(&f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Number(value) = value {
                handler(&value, window, cx);
            }
        }));
        self
    }

    fn layout(&self) -> Style {
        let mtm = MainThreadMarker::new().expect("gpui builds its frames on the main thread");
        let style = NSScroller::preferredScrollerStyle(mtm);
        fitted(fitting_size_keyed("NSScroller", style.0 as u64, |mtm| {
            NSSize::new(
                NSScroller::scrollerWidthForControlSize_scrollerStyle(
                    NSControlSize::Regular,
                    style,
                    mtm,
                ),
                0.,
            )
        }))
    }

    fn paint(&mut self, slot: Slot, window: &mut Window, _: &mut App) {
        let held = window.with_element_state::<Held, _>(slot.id, |state, _| {
            let state = state.unwrap_or_default();
            (state.clone(), state)
        });
        let fresh = Cell::new(false);
        let scroller = slot.place(Layer::Above, |mtm, target| {
            let scroller = Scroller::new(mtm);
            wire(&scroller, target, slot.key);
            fresh.set(true);
            scroller
        });
        if fresh.get() {
            held.0.set(None);
        }

        let style = NSScroller::preferredScrollerStyle(scroller.mtm());
        if scroller.scrollerStyle() != style {
            scroller.setScrollerStyle(style);
            scroller.flash();
        }

        let wanted = (self.value.clamp(0., 1.), self.proportion.clamp(0., 1.));
        let was = held.0.get();
        if was != Some(wanted) {
            scroller.setKnobProportion(wanted.1);
            scroller.setDoubleValue(wanted.0);
            held.0.set(Some(wanted));
            // An overlay scroller shows itself when the content moves under it, never because
            // the content grew: a listing that is still landing draws no scroller in Finder.
            if was.is_some_and(|(value, _)| value != wanted.0) {
                scroller.flash();
            }
        }

        let handler = self.on_change.take().map(|handler| -> NativeDispatch {
            let held = held.0.clone();
            Rc::new(move |value, window, cx| {
                let Ok(value) = value.downcast::<NativeValue>() else {
                    return;
                };
                if let NativeValue::Number(number) = *value {
                    held.set(held.get().map(|(_, proportion)| (number, proportion)));
                }
                handler(*value, window, cx);
            })
        });
        slot.on_event(reader(), handler);
    }
}

hosted_element!(NativeScroller);

/// Where the scroller says it is now: the knob carries its own value, a click in the track pages
/// by what the knob covers.
fn reader() -> NativeRead {
    Rc::new(|sender| {
        let part: usize = unsafe { msg_send![sender, hitPart] };
        let value: f64 = unsafe { msg_send![sender, doubleValue] };
        let proportion: f64 = unsafe { msg_send![sender, knobProportion] };
        let page = if proportion < 1. {
            proportion / (1. - proportion)
        } else {
            1.
        };
        let value = match NSScrollerPart(part) {
            NSScrollerPart::DecrementPage => value - page,
            NSScrollerPart::IncrementPage => value + page,
            _ => value,
        };
        Box::new(NativeValue::Number(value.clamp(0., 1.)))
    })
}

define_class!(
    /// Outside an `NSScrollView` nothing drives an overlay scroller's fade but itself. Faded out
    /// it takes no mouse: that edge is the content's again.
    #[unsafe(super(NSScroller, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGScroller"]
    struct Scroller;

    impl Scroller {
        // AppKit's own answer is `self == NSScroller`, so a subclass is refused the overlay style
        // and drawn as the legacy one however the system is set.
        #[unsafe(method(isCompatibleWithOverlayScrollers))]
        fn compatible_with_overlay_scrollers() -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            if self.alphaValue() <= 0. {
                return std::ptr::null_mut();
            }
            unsafe { msg_send![super(self), hitTest: point] }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.hold();
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            self.flash();
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if let Some(next) = unsafe { self.nextResponder() } {
                next.scrollWheel(event);
            }
        }

        #[unsafe(method(ngStyleChanged))]
        fn style_changed(&self) {
            self.setScrollerStyle(NSScroller::preferredScrollerStyle(self.mtm()));
            self.flash();
        }

        #[unsafe(method(ngFade))]
        fn fade(&self) {
            NSAnimationContext::beginGrouping();
            NSAnimationContext::currentContext().setDuration(FADE);
            self.animator().setAlphaValue(0.);
            NSAnimationContext::endGrouping();
        }
    }
);

impl Scroller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // A scroller takes its axis from the frame it is made with, and never changes it.
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(1., 2.));
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] };
        // A scroller AppKit did not make for a scroll view starts disabled, and draws nothing.
        this.setEnabled(true);
        this.setScrollerStyle(NSScroller::preferredScrollerStyle(mtm));
        if this.scrollerStyle() == NSScrollerStyle::Overlay {
            this.setAlphaValue(0.);
        }
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &this,
                sel!(ngStyleChanged),
                Some(NSPreferredScrollerStyleDidChangeNotification),
                None,
            );
        }
        this
    }

    fn hold(&self) {
        unsafe {
            NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(
                self,
                sel!(ngFade),
                None,
            );
        }
        self.setAlphaValue(1.);
    }

    fn flash(&self) {
        self.hold();
        if self.scrollerStyle() == NSScrollerStyle::Overlay {
            unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: sel!(ngFade),
                    withObject: std::ptr::null::<AnyObject>(),
                    afterDelay: LINGER,
                ];
            }
        }
    }
}
