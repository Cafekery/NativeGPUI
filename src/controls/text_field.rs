//! `NSTextField`, `NSSecureTextField` and `NSSearchField`.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, ElementId, SharedString, Style, Window, relative};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSSearchField, NSSearchFieldDelegate,
    NSSecureTextField, NSTextField, NSTextFieldDelegate, NSTextView,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNumber, NSObject, NSRange, NSSize, NSString,
};

use crate::bridge::appkit::fitting_size;
use crate::bridge::host::{
    Layer, NativeDispatch, NativeHandler, NativeValue, Slot, Target, ValueKind, hosted_element,
    wire,
};
use crate::controls::fitted;

/// Salts the element key into a second tag, so Return and per-keystroke change are separate
/// channels on one pooled view.
const SUBMIT_CHANNEL: u64 = 0x9e37_79b9_7f4a_7c15;
const CANCEL_CHANNEL: u64 = 0x6a09_e667_f3bc_c908;

/// `text` is the value you own; wire `on_change` to keep it. ⌘A/⌘C/⌘V/⌘X/⌘Z reach a hosted field
/// only after [`crate::install_menus`].
pub fn native_text_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Plain, text)
}

pub fn native_secure_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Secure, text)
}

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
    stretch: bool,
    /// Take the keyboard on appearing, selecting these characters.
    editing: Option<std::ops::Range<usize>>,
    on_change: Option<NativeHandler>,
    on_submit: Option<NativeHandler>,
    on_cancel: Option<NativeHandler>,
}

impl NativeTextField {
    fn new(id: ElementId, kind: Kind, text: &str) -> Self {
        Self {
            id,
            kind,
            text: SharedString::from(text.to_owned()),
            placeholder: SharedString::default(),
            enabled: true,
            stretch: false,
            editing: None,
            on_change: None,
            on_submit: None,
            on_cancel: None,
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

    /// Every keystroke, including IME commits, paste and dictation.
    pub fn on_change(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(text_handler(handler));
        self
    }

    /// Return only. A search field's cancel button clears the text and fires both handlers with
    /// `""`.
    pub fn on_submit(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_submit = Some(text_handler(handler));
        self
    }

    /// Escape while editing; the text is whatever was typed so far.
    pub fn on_cancel(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(text_handler(handler));
        self
    }

    /// Fill the box's width rather than the field's own fitting width.
    pub fn stretch(mut self) -> Self {
        self.stretch = true;
        self
    }

    /// Become first responder on appearing, with these characters selected, as a field that
    /// renames in place does.
    pub fn editing(mut self, selected: std::ops::Range<usize>) -> Self {
        self.editing = Some(selected);
        self
    }

    fn layout(&self) -> Style {
        let kind = self.kind;
        let mut style = fitted(fitting_size(kind.class(), |mtm| kind.fitting(mtm)));
        if self.stretch {
            style.size.width = relative(1.).into();
        }
        style
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let submit = slot.key ^ SUBMIT_CHANNEL;
        // Each class is its own `V`, so a field that changes kind is rebuilt by the host.
        let field: Retained<NSTextField> = match self.kind {
            Kind::Plain => slot.place(Layer::Above, |mtm, target| {
                let field = NSTextField::new(mtm);
                configure(&field, target, submit);
                field
            }),
            Kind::Secure => slot
                .place(Layer::Above, |mtm, target| {
                    let field = NSSecureTextField::new(mtm);
                    configure(&field, target, submit);
                    field
                })
                .into_super(),
            Kind::Search => slot
                .place(Layer::Above, |mtm, target| {
                    let field = NSSearchField::new(mtm);
                    // Action on Return, not on every pause in typing.
                    field.setSendsWholeSearchString(true);
                    field.setSendsSearchStringImmediately(false);
                    configure(&field, target, submit);
                    field
                })
                .into_super(),
        };

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
        // Events are a frame late, so `self.text` is stale for a frame or two after a keystroke;
        // writing it back then would delete what the user typed in between.
        let want_text: &str = &self.text;
        if pending(&field) == 0 && field.stringValue().to_string() != want_text {
            field.setStringValue(&NSString::from_str(want_text));
        }

        let acked = field.retain();
        let user = self.on_change.take();
        slot.on_change(
            ValueKind::Text,
            Some(Rc::new(move |value, window, cx| {
                set_pending(&acked, pending(&acked) - 1);
                if let Some(user) = &user {
                    user(value, window, cx);
                }
            })),
        );
        let dispatch = |handler: NativeHandler| -> NativeDispatch {
            Rc::new(move |value, window, cx| {
                if let Ok(value) = value.downcast::<NativeValue>() {
                    handler(*value, window, cx);
                }
            })
        };
        let submit_handler = self.on_submit.take().map(dispatch);
        slot.host
            .on_event_channel(slot.key, submit, ValueKind::Text.reader(), submit_handler);
        let cancel_handler = self.on_cancel.take().map(dispatch);
        slot.host.on_event_channel(
            slot.key,
            slot.key ^ CANCEL_CHANNEL,
            ValueKind::Text.reader(),
            cancel_handler,
        );

        if let Some(selected) = &self.editing {
            if field.currentEditor().is_none() {
                if let Some(window) = field.window() {
                    // A field in a panel that is not key edits nothing: take key first.
                    if !window.isKeyWindow() && window.canBecomeKeyWindow() {
                        window.makeKeyWindow();
                    }
                    window.makeFirstResponder(Some(&field));
                }
                if let Some(editor) = field.currentEditor() {
                    let start = selected.start.min(want_text.chars().count());
                    let len = selected.end.saturating_sub(selected.start);
                    editor.setSelectedRange(NSRange::new(start, len));
                }
            }
        }
    }
}

hosted_element!(NativeTextField);

fn text_handler(handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> NativeHandler {
    Rc::new(move |value, window, cx| {
        if let NativeValue::Text(text) = value {
            handler(&text, window, cx);
        }
    })
}

fn configure(field: &NSTextField, target: &Target, submit: u64) {
    field.setUsesSingleLineMode(true);
    if let Some(cell) = field.cell() {
        cell.setSendsActionOnEndEditing(false);
    }
    wire(field, target, submit);
    let delegate = shared_delegate(MainThreadMarker::from(field));
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
}

define_class!(
    /// One delegate for every hosted field: `NSTextField` fires its action on Return only, so
    /// per-keystroke change comes from `controlTextDidChange:`.
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
            // The host dispatches on the sender's tag, so borrow it for one send.
            let submit = control.tag();
            control.setTag(submit ^ SUBMIT_CHANNEL as isize);
            let _: bool = unsafe { msg_send![control, sendAction: action, to: &*target] };
            control.setTag(submit);
        }

        /// Escape reaches the field editor as `cancelOperation:`; sent on as the cancel channel.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, control: &NSControl, _: &NSTextView, selector: Sel) -> Bool {
            if selector != sel!(cancelOperation:) {
                return Bool::NO;
            }
            let (Some(action), Some(target)) = (control.action(), control.target()) else {
                return Bool::NO;
            };
            let submit = control.tag();
            control.setTag(submit ^ SUBMIT_CHANNEL as isize ^ CANCEL_CHANNEL as isize);
            let _: bool = unsafe { msg_send![control, sendAction: action, to: &*target] };
            control.setTag(submit);
            Bool::YES
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

/// Edits dispatched to GPUI that GPUI has not applied yet, kept in the cell's `representedObject`.
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
