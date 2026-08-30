//! `NSTextField`, `NSSecureTextField` and `NSSearchField`.

use std::cell::RefCell;
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, SharedString, Style, Window, px, relative,
};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSSearchField, NSSearchFieldDelegate,
    NSSecureTextField, NSTextField, NSTextFieldDelegate,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSNumber, NSObject, NSSize, NSString};

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHandler, NativeHost, NativeValue, Target, ValueKind, element_key,
    wire,
};

/// Salts the element key into a second tag, so Return and per-keystroke change reach different
/// [`NativeHost::on_event_channel`] entries while both retire with the one pooled view.
const SUBMIT_CHANNEL: u64 = 0x9e37_79b9_7f4a_7c15;

/// `text` is the value you own; wire [`NativeTextField::on_change`] to keep it.
///
/// ⌘A/⌘C/⌘V/⌘X/⌘Z reach a hosted field only after [`crate::install_menus`]. macOS takes those key
/// equivalents from the main menu and a GPUI app installs none, so the chord is offered to the
/// view hierarchy, declined, offered to an absent menu bar, and dropped. GPUI's own view does see
/// `performKeyEquivalent:` first, but it declines every chord its keymap does not bind.
///
/// Arrows and forward-delete need no opt-in: the host adopts the window's key path when it places
/// a view. Without that, GPUI answers every characterless key through its input context, which
/// resolves to the focused field editor and hands it `insertText:` with the legacy ASCII spelling
/// — ← as U+001C, → as U+001D, forward-delete as U+007F — so `moveLeft:` never ran.
///
/// Home and End do not move the caret, here or in an unhosted field: the editor answers them with
/// `scrollToBeginningOfDocument:`/`scrollToEndOfDocument:`, which leave the insertion point alone.
pub fn native_text_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Plain, text)
}

/// Bullets, and the system's secure input path.
pub fn native_secure_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Secure, text)
}

/// Magnifier and cancel button included.
pub fn native_search_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Search, text)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Plain,
    Secure,
    Search,
}

impl Kind {
    fn class(self) -> &'static str {
        match self {
            Kind::Plain => "NSTextField",
            Kind::Secure => "NSSecureTextField",
            Kind::Search => "NSSearchField",
        }
    }

    fn fitting(self, mtm: MainThreadMarker) -> NSSize {
        match self {
            Kind::Plain => NSTextField::new(mtm).fittingSize(),
            Kind::Secure => NSSecureTextField::new(mtm).fittingSize(),
            Kind::Search => NSSearchField::new(mtm).fittingSize(),
        }
    }
}

pub struct NativeTextField {
    id: ElementId,
    kind: Kind,
    text: SharedString,
    placeholder: SharedString,
    enabled: bool,
    on_change: Option<NativeHandler>,
    on_submit: Option<NativeHandler>,
}

impl NativeTextField {
    fn new(id: ElementId, kind: Kind, text: &str) -> Self {
        Self {
            id,
            kind,
            text: SharedString::from(text.to_owned()),
            placeholder: SharedString::default(),
            enabled: true,
            on_change: None,
            on_submit: None,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Every keystroke, including IME commits, paste, dictation and the emoji picker.
    pub fn on_change(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Text(text) = value {
                handler(&text, window, cx);
            }
        }));
        self
    }

    /// Return only — losing focus does not submit. A search field's cancel button is AppKit's
    /// own exception: it clears the text and sends the action, so both handlers fire with `""`.
    pub fn on_submit(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_submit = Some(Rc::new(move |value, window, cx| {
            if let NativeValue::Text(text) = value {
                handler(&text, window, cx);
            }
        }));
        self
    }

    /// Everything that is identical across the three classes, applied through `NSTextField`.
    fn sync(&mut self, field: &NSTextField, host: &NativeHost, key: u64) {
        if field.isEnabled() != self.enabled {
            field.setEnabled(self.enabled);
        }
        let want_placeholder: &str = &self.placeholder;
        let matches = match field.placeholderString() {
            Some(have) => have.to_string() == want_placeholder,
            None => want_placeholder.is_empty(),
        };
        if !matches {
            field.setPlaceholderString(Some(&NSString::from_str(want_placeholder)));
        }

        // Events are a frame late, so `self.text` is stale for a frame or two after every
        // keystroke: writing it back then would delete what the user typed in between.
        let want_text: &str = &self.text;
        if pending(field) == 0 && field.stringValue().to_string() != want_text {
            field.setStringValue(&NSString::from_str(want_text));
        }

        let acked = field.retain();
        let user = self.on_change.take();
        host.on_change(
            key,
            ValueKind::Text,
            Some(Rc::new(move |value, window, cx| {
                set_pending(&acked, pending(&acked) - 1);
                if let Some(user) = &user {
                    user(value, window, cx);
                }
            })),
        );
        let submit = self.on_submit.take().map(|handler| -> NativeDispatch {
            Rc::new(move |value, window, cx| {
                if let Ok(value) = value.downcast::<NativeValue>() {
                    handler(*value, window, cx);
                }
            })
        });
        host.on_event_channel(key, key ^ SUBMIT_CHANNEL, ValueKind::Text.reader(), submit);
    }
}

/// One-time setup shared by all three classes. `submit` is the tag the native action carries;
/// the delegate substitutes the change key on top of it per keystroke.
fn configure(field: &NSTextField, target: &Target, submit: u64) {
    field.setUsesSingleLineMode(true);
    if let Some(cell) = field.cell() {
        // Submit means Return, not "the user tabbed away".
        cell.setSendsActionOnEndEditing(false);
    }
    wire(field, target, submit);
    let delegate = shared_delegate(MainThreadMarker::from(field));
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
}

impl Element for NativeTextField {
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
        let kind = self.kind;
        let fitting = fitting_size(kind.class(), |mtm| kind.fitting(mtm));
        let mut style = Style::default();
        // A zero axis from `fittingSize` means AppKit declares that axis free; all three classes
        // currently report no intrinsic width, so the caller's layout drives it.
        style.size.width = if fitting.width > 0. {
            px(fitting.width as f32).into()
        } else {
            relative(1.).into()
        };
        style.size.height = px(fitting.height as f32).into();
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
        let submit = key ^ SUBMIT_CHANNEL;
        let mask = window.content_mask().bounds;
        let host = NativeHost::get(window, cx);

        // Each class is its own `V`, so a field that changes kind under one element id is
        // rebuilt by the host rather than reinterpreted.
        match self.kind {
            Kind::Plain => {
                let field = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
                    let field = NSTextField::new(mtm);
                    configure(&field, target, submit);
                    field
                });
                self.sync(&field, &host, key);
            }
            Kind::Secure => {
                let field = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
                    let field = NSSecureTextField::new(mtm);
                    configure(&field, target, submit);
                    field
                });
                self.sync(&field, &host, key);
            }
            Kind::Search => {
                let field = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
                    let field = NSSearchField::new(mtm);
                    // Action on Return, not on every pause in typing — that is `on_change`'s job.
                    field.setSendsWholeSearchString(true);
                    field.setSendsSearchStringImmediately(false);
                    configure(&field, target, submit);
                    field
                });
                self.sync(&field, &host, key);
            }
        }
    }
}

impl IntoElement for NativeTextField {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

define_class!(
    /// One delegate for every hosted field in the process: `NSTextField` sends its target/action
    /// on Return only, so per-keystroke change has to come from `controlTextDidChange:`. The
    /// notification names the control, so no per-field state is needed.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGFieldDelegate"]
    struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(object) = notification.object() else {
                return;
            };
            let Some(control) = object.downcast_ref::<NSControl>() else {
                return;
            };
            let (Some(action), Some(target)) = (control.action(), control.target()) else {
                return;
            };

            if let Some(field) = control.downcast_ref::<NSTextField>() {
                set_pending(field, pending(field) + 1);
            }

            // The host dispatches on the sender's tag and reads the value synchronously, so
            // borrowing the tag for one send is enough to reach the change entry.
            let submit = control.tag();
            control.setTag(submit ^ SUBMIT_CHANNEL as isize);
            let _: bool = unsafe { msg_send![control, sendAction: action, to: &*target] };
            control.setTag(submit);
        }
    }

    unsafe impl NSTextFieldDelegate for FieldDelegate {}
    unsafe impl NSSearchFieldDelegate for FieldDelegate {}
);

fn shared_delegate(mtm: MainThreadMarker) -> Retained<FieldDelegate> {
    thread_local! {
        static DELEGATE: RefCell<Option<Retained<FieldDelegate>>> = const { RefCell::new(None) };
    }
    DELEGATE.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| unsafe { msg_send![FieldDelegate::alloc(mtm), init] })
            .clone()
    })
}

/// Edits dispatched to GPUI that GPUI has not applied yet — nonzero means the field owns its own
/// text and `paint` must not touch `stringValue`. Kept in the cell's `representedObject` so its
/// lifetime is exactly the field's.
fn pending(field: &NSTextField) -> isize {
    field
        .cell()
        .and_then(|cell| cell.representedObject())
        .and_then(|object| object.downcast::<NSNumber>().ok())
        .map_or(0, |count| count.as_isize())
}

fn set_pending(field: &NSTextField, count: isize) {
    if let Some(cell) = field.cell() {
        unsafe { cell.setRepresentedObject(Some(&NSNumber::new_isize(count.max(0)))) };
    }
}
