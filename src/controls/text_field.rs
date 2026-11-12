use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, ElementId, SharedString, Style, Window, relative};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSComboBox, NSComboBoxDelegate, NSControl, NSControlTextEditingDelegate, NSFocusRingType,
    NSSearchField, NSSearchFieldDelegate, NSSecureTextField, NSTextField, NSTextFieldDelegate,
    NSTextView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSNumber, NSObject, NSRange, NSString,
};

use crate::bridge::host::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeRead, NativeValue, Slot, Target,
    ValueKind, hosted_element, wire,
};
use crate::controls::{
    measured, nsstring_is, sync_control_size, sync_enabled, sync_font_size, with_width,
};

const CHANGE_CHANNEL: u64 = 0;
const SUBMIT_CHANNEL: u64 = 0x9e37_79b9_7f4a_7c15;
const CANCEL_CHANNEL: u64 = 0x6a09_e667_f3bc_c908;
const COMMAND_CHANNEL: u64 = 0xbb67_ae85_84ca_a73b;

/// `text` must be the state `on_change` writes; edit chords need [`crate::install_menus`].
pub fn native_text_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Plain, text)
}

pub fn native_secure_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Secure, text)
}

pub fn native_search_field(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Search, text)
}

/// A field with a pop-up list of [`NativeTextField::items`]; picking one fires `on_change`.
pub fn native_combo_box(id: impl Into<ElementId>, text: &str) -> NativeTextField {
    NativeTextField::new(id.into(), Kind::Combo, text)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Kind {
    Plain,
    Secure,
    Search,
    Combo,
}

impl Kind {
    fn class(self) -> &'static str {
        match self {
            Kind::Plain => "NSTextField",
            Kind::Secure => "NSSecureTextField",
            Kind::Search => "NSSearchField",
            Kind::Combo => "NSComboBox",
        }
    }
}

pub struct NativeTextField {
    id: ElementId,
    kind: Kind,
    text: SharedString,
    placeholder: SharedString,
    items: Vec<SharedString>,
    enabled: bool,
    stretch: bool,
    borderless: bool,
    control_size: ControlSize,
    font_size: Option<f32>,
    editing: Option<std::ops::Range<usize>>,
    on_change: Option<NativeHandler>,
    on_submit: Option<NativeHandler>,
    on_cancel: Option<NativeHandler>,
    commands: &'static [&'static str],
    on_command: Option<NativeDispatch>,
}

impl NativeTextField {
    fn new(id: ElementId, kind: Kind, text: &str) -> Self {
        Self {
            id,
            kind,
            text: SharedString::from(text.to_owned()),
            placeholder: SharedString::default(),
            items: Vec::new(),
            control_size: ControlSize::default(),
            font_size: None,
            enabled: true,
            stretch: false,
            borderless: false,
            editing: None,
            on_change: None,
            on_submit: None,
            on_cancel: None,
            commands: &[],
            on_command: None,
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

    /// A combo box's list.
    pub fn items(mut self, items: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(text_handler(handler));
        self
    }

    /// Return. A search field's cancel button clears the text and fires both handlers with `""`.
    pub fn on_submit(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_submit = Some(text_handler(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(text_handler(handler));
        self
    }

    /// `moveRight:` is only taken with the caret at the end of the text.
    pub fn on_command(
        mut self,
        commands: &'static [&'static str],
        handler: impl Fn(&str, &String, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.commands = commands;
        self.on_command = Some(Rc::new(move |value: Box<dyn Any>, window, cx| {
            if let Ok(value) = value.downcast::<(String, String)>() {
                let (command, text) = *value;
                handler(&command, &text, window, cx);
            }
        }));
        self
    }

    pub fn stretch(mut self) -> Self {
        self.stretch = true;
        self
    }

    pub fn borderless(mut self) -> Self {
        self.borderless = true;
        self
    }

    /// Takes first responder on appearing, with these characters selected.
    pub fn editing(mut self, selected: std::ops::Range<usize>) -> Self {
        self.editing = Some(selected);
        self
    }

    pub fn control_size(mut self, control_size: ControlSize) -> Self {
        self.control_size = control_size;
        self
    }

    /// The system font at this point size, in place of the control size's own; the height follows.
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    fn layout(&self) -> Style {
        let inputs = (self.control_size, self.font_size.map(f32::to_bits));
        let style = measured(self.kind.class(), inputs, |mtm| {
            let field: Retained<NSTextField> = match self.kind {
                Kind::Plain => NSTextField::new(mtm),
                Kind::Secure => Retained::into_super(NSSecureTextField::new(mtm)),
                Kind::Search => Retained::into_super(NSSearchField::new(mtm)),
                Kind::Combo => Retained::into_super(NSComboBox::new(mtm)),
            };
            field.setControlSize(self.control_size.into());
            sync_font_size(&field, self.font_size.map(f64::from));
            field.fittingSize()
        });
        with_width(style, self.stretch.then(|| relative(1.).into()))
    }

    fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
        let submit = slot.key ^ SUBMIT_CHANNEL;
        // One `V` per class, so the host rebuilds a field that changes kind.
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
                    field.setSendsWholeSearchString(true);
                    field.setSendsSearchStringImmediately(false);
                    configure(&field, target, submit);
                    field
                })
                .into_super(),
            Kind::Combo => {
                let combo = slot.place(Layer::Above, |mtm, target| {
                    let combo = NSComboBox::new(mtm);
                    combo.setCompletes(true);
                    configure(&combo, target, submit);
                    combo
                });
                sync_items(&combo, &self.items);
                combo.into_super()
            }
        };

        sync_control_size(&field, self.control_size);
        sync_font_size(&field, self.font_size.map(f64::from));
        sync_enabled(&field, self.enabled);
        if self.borderless && field.drawsBackground() {
            field.setBezeled(false);
            field.setBordered(false);
            field.setDrawsBackground(false);
            field.setFocusRingType(NSFocusRingType::None);
        }
        let placeholder_matches = field
            .placeholderString()
            .map_or(self.placeholder.is_empty(), |have| {
                nsstring_is(&have, &self.placeholder)
            });
        if !placeholder_matches {
            field.setPlaceholderString(Some(&NSString::from_str(&self.placeholder)));
        }
        // `self.text` lags typing by the pending edits; writing it back would eat keystrokes.
        let want_text: &str = &self.text;
        if pending(&field) == 0 && !nsstring_is(&field.stringValue(), want_text) {
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
        let cancel = slot.key ^ CANCEL_CHANNEL;
        let host = &slot.host;
        host.on_change_channel(slot.key, submit, ValueKind::Text, self.on_submit.take());
        host.on_change_channel(slot.key, cancel, ValueKind::Text, self.on_cancel.take());
        let address = Retained::as_ptr(&field) as usize;
        COMMANDS.with_borrow_mut(|taken| {
            if self.on_command.is_some() {
                taken.insert(address, self.commands);
            } else {
                taken.remove(&address);
            }
        });
        host.on_event_channel(
            slot.key,
            slot.key ^ COMMAND_CHANNEL,
            command_read(),
            self.on_command.take(),
        );

        let Some(selected) = &self.editing else {
            return;
        };
        if field.currentEditor().is_some() {
            return;
        }
        if let Some(window) = field.window() {
            if !window.isKeyWindow() && window.canBecomeKeyWindow() {
                window.makeKeyWindow();
            }
            window.makeFirstResponder(Some(&field));
        }
        if let Some(editor) = field.currentEditor() {
            // `editing` counts chars; AppKit ranges count UTF-16 units.
            let units = |chars: usize| {
                want_text
                    .chars()
                    .take(chars)
                    .map(char::len_utf16)
                    .sum::<usize>()
            };
            let (start, end) = (units(selected.start), units(selected.end));
            editor.setSelectedRange(NSRange::new(start, end.saturating_sub(start)));
        }
    }
}

hosted_element!(NativeTextField);

thread_local! {
    static COMMANDS: RefCell<HashMap<usize, &'static [&'static str]>> = RefCell::new(HashMap::new());
    static LAST_COMMAND: RefCell<String> = const { RefCell::new(String::new()) };
}

fn command_read() -> NativeRead {
    Rc::new(|sender: &AnyObject| {
        let text: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
        let command = LAST_COMMAND.with(|last| last.borrow().clone());
        Box::new((command, text.to_string()))
    })
}

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
        // An `init`-made cell wraps a long line onto a second one, single-line mode or not.
        cell.setScrollable(true);
    }
    wire(field, target, submit);
    let delegate = shared_delegate(MainThreadMarker::from(field));
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
}

fn sync_items(combo: &NSComboBox, items: &[SharedString]) {
    let have = combo.objectValues();
    let same = have.len() == items.len()
        && have.iter().zip(items).all(|(had, item)| {
            had.downcast_ref::<NSString>()
                .is_some_and(|had| nsstring_is(had, item))
        });
    if same {
        return;
    }
    combo.removeAllItems();
    let strings: Vec<_> = items.iter().map(|item| NSString::from_str(item)).collect();
    let strings: Retained<NSArray> =
        unsafe { Retained::cast_unchecked(NSArray::from_retained_slice(&strings)) };
    unsafe { combo.addItemsWithObjectValues(&strings) };
}

fn text_changed(notification: &NSNotification) {
    if let Some(control) = notification
        .object()
        .as_deref()
        .and_then(|object| object.downcast_ref::<NSControl>())
    {
        send_on(control, CHANGE_CHANNEL);
    }
}

fn send_on(control: &NSControl, channel: u64) -> Bool {
    let (Some(action), Some(target)) = (control.action(), control.target()) else {
        return Bool::NO;
    };
    if channel == CHANGE_CHANNEL {
        if let Some(field) = control.downcast_ref::<NSTextField>() {
            set_pending(field, pending(field) + 1);
        }
    }
    let submit = control.tag();
    control.setTag(submit ^ SUBMIT_CHANNEL as isize ^ channel as isize);
    let _: bool = unsafe { msg_send![control, sendAction: action, to: &*target] };
    control.setTag(submit);
    Bool::YES
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGFieldDelegate"]
    struct FieldDelegate;

    unsafe impl NSObjectProtocol for FieldDelegate {}

    unsafe impl NSControlTextEditingDelegate for FieldDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            text_changed(notification);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, control: &NSControl, text_view: &NSTextView, selector: Sel) -> Bool {
            if selector == sel!(cancelOperation:) {
                return send_on(control, CANCEL_CHANNEL);
            }
            let name = selector.name().to_str().unwrap_or_default();
            let address = control as *const NSControl as usize;
            let taken = COMMANDS.with(|taken| {
                taken
                    .borrow()
                    .get(&address)
                    .is_some_and(|commands| commands.contains(&name))
            });
            if !taken {
                return Bool::NO;
            }
            if selector == sel!(moveRight:) {
                let selected = text_view.selectedRange();
                if selected.length != 0 || selected.location != text_view.string().length() {
                    return Bool::NO;
                }
            }
            LAST_COMMAND.with(|last| *last.borrow_mut() = name.to_owned());
            send_on(control, COMMAND_CHANNEL)
        }
    }

    unsafe impl NSTextFieldDelegate for FieldDelegate {}
    unsafe impl NSSearchFieldDelegate for FieldDelegate {}

    unsafe impl NSComboBoxDelegate for FieldDelegate {
        /// Fires before the combo box shows the item, so the item is written first.
        #[unsafe(method(comboBoxSelectionDidChange:))]
        fn combo_box_selection_did_change(&self, notification: &NSNotification) {
            let Some(object) = notification.object() else {
                return;
            };
            let Some(combo) = object.downcast_ref::<NSComboBox>() else {
                return;
            };
            let Some(picked) = combo
                .objectValueOfSelectedItem()
                .and_then(|picked| picked.downcast::<NSString>().ok())
            else {
                return;
            };
            combo.setStringValue(&picked);
            text_changed(notification);
        }
    }
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
