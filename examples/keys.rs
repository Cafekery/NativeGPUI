//! Keyboard behaviour in a window full of hosted AppKit views, measured rather than eyeballed.
//!
//! Every assertion reads AppKit's own state on the main thread — `selectedRange` off the field
//! editor, `stringValue` off the field, the real `NSPasteboard`, `NSWindow.firstResponder` — so no
//! result here depends on a screenshot. Keystrokes are equally real: this process never
//! synthesises an `NSEvent`. It writes an AppleScript fragment to a handshake directory and waits
//! for `scripts/keys.sh` to hand it to System Events, which is the same path a physical key takes
//! from the WindowServer into `NSApplication.sendEvent:`.
//!
//! Three hedges against a green run that means nothing:
//!
//! * A row whose pre-state already equals its expectation reports `VACUOUS`, never `PASS` — the
//!   keystroke cannot have been the cause of a state it was already in.
//! * A row measured while this window was not key, or while the OS reported another app frontmost,
//!   reports `NOKEY` and is not counted as a result.
//! * `KEYS_SABOTAGE=want` inverts every expectation, so every row must go red; and
//!   `KEYS_SABOTAGE=nokeys` runs the entire handshake while delivering no keystroke at all, so no
//!   measuring row may pass. Either arm coming back green is a harness bug, not a finding.
//!
//! `KEYS_MENUS=0` skips [`native_gpui::install_menus`], which is the arm the ⌘ keys were broken in.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, Application, AsyncApp, Bounds, Context, FocusHandle, MouseDownEvent, Render,
    SharedString, Window, WindowBounds, WindowHandle, div, point, prelude::*, px, rgb, size,
};
use native_gpui::{
    native_button, native_root, native_slider, native_switch, native_text_field, window_options,
};
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{ClassType, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSPasteboard, NSPasteboardTypeString, NSScreen, NSSlider, NSSwitch, NSTextField, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSPoint, NSProcessInfo, NSRange, NSRect, NSSize, NSString,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Long enough for System Events' key to reach `sendEvent:` and for one GPUI frame to commit after
/// it. Every state read is of AppKit, which is synchronous, so this only has to cover delivery.
const SETTLE: Duration = Duration::from_millis(160);
const POLL: Duration = Duration::from_millis(15);

// ---------------------------------------------------------------------------------------------
// the window under test

struct Keys {
    alpha: String,
    beta: String,
    toggle: bool,
    slider: f64,
    default_clicks: u32,
    plain_clicks: u32,
    submits: u32,
    gpui_clicks: u32,
    focus: FocusHandle,
}

impl Render for Keys {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        native_root(
            div()
                .track_focus(&self.focus)
                .size_full()
                .bg(rgb(0x1c1c1e))
                .text_color(rgb(0xf2f2f7))
                .text_size(px(11.))
                .flex()
                .child(
                    div()
                        .w(px(380.))
                        .p(px(24.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div().w(px(300.)).child(
                                native_text_field("alpha", &self.alpha)
                                    .placeholder("alpha")
                                    .on_change(cx.listener(|this, text: &String, _, cx| {
                                        this.alpha = text.clone();
                                        cx.notify();
                                    }))
                                    .on_submit(cx.listener(|this, _: &String, _, cx| {
                                        this.submits += 1;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(
                            div().w(px(300.)).child(
                                native_text_field("beta", &self.beta)
                                    .placeholder("beta")
                                    .on_change(cx.listener(|this, text: &String, _, cx| {
                                        this.beta = text.clone();
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(16.))
                                .child(native_switch("toggle", self.toggle).on_change(cx.listener(
                                    |this, on: &bool, _, cx| {
                                        this.toggle = *on;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    div().w(px(200.)).child(
                                        native_slider("slider", self.slider)
                                            .range(0., 100.)
                                            .on_change(cx.listener(|this, value: &f64, _, cx| {
                                                this.slider = *value;
                                                cx.notify();
                                            })),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .child(native_button("go", "Go").default_button(true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.default_clicks += 1;
                                        cx.notify();
                                    }),
                                ))
                                .child(native_button("plain", "Plain").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.plain_clicks += 1;
                                        cx.notify();
                                    },
                                ))),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .bg(rgb(0x2c2c2e))
                        .p(px(24.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                this.gpui_clicks += 1;
                                cx.notify();
                            }),
                        )
                        .child(SharedString::from(format!(
                            "gpui clicks {} · default {} · plain {} · submits {}",
                            self.gpui_clicks, self.default_clicks, self.plain_clicks, self.submits
                        ))),
                ),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// the AppKit objects the suite reads

struct Controls {
    window: Retained<NSWindow>,
    gpui: Retained<NSView>,
    alpha: Retained<NSTextField>,
    beta: Retained<NSTextField>,
    toggle: Retained<NSSwitch>,
    slider: Retained<NSSlider>,
}

fn descend(view: &NSView, out: &mut Vec<Retained<NSView>>) {
    let subviews = view.subviews();
    for index in 0..subviews.count() {
        let sub = subviews.objectAtIndex(index);
        descend(&sub, out);
        out.push(sub);
    }
}

/// `isKindOfClass` rather than a downcast: AppKit's classes are main-thread-only and objc2 will not
/// hand out a checked cast for them, so the class test has to be explicit.
fn as_kind<T: objc2::Message + objc2::ClassType>(view: &Retained<NSView>) -> Option<Retained<T>> {
    if view.isKindOfClass(T::class()) {
        Some(unsafe { Retained::cast_unchecked::<T>(view.clone()) })
    } else {
        None
    }
}

impl Controls {
    fn find(window: &Window) -> Option<Self> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        let gpui = unsafe { Retained::retain(appkit.ns_view.as_ptr().cast::<NSView>()) }?;
        let ns_window = gpui.window()?;
        let content = ns_window.contentView()?;

        let mut all = Vec::new();
        descend(&content, &mut all);

        let field = |placeholder: &str| {
            all.iter().filter_map(as_kind::<NSTextField>).find(|f| {
                f.placeholderString()
                    .is_some_and(|have| have.to_string() == placeholder)
            })
        };
        Some(Self {
            alpha: field("alpha")?,
            beta: field("beta")?,
            toggle: all.iter().filter_map(as_kind::<NSSwitch>).next()?,
            slider: all.iter().filter_map(as_kind::<NSSlider>).next()?,
            window: ns_window,
            gpui,
        })
    }
}

fn same(a: &impl objc2::Message, b: &impl objc2::Message) -> bool {
    std::ptr::eq(a as *const _ as *const (), b as *const _ as *const ())
}

fn board() -> String {
    NSPasteboard::generalPasteboard()
        .stringForType(unsafe { NSPasteboardTypeString })
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn set_board(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
}

// ---------------------------------------------------------------------------------------------
// results

#[derive(Copy, Clone, PartialEq, Eq)]
enum Sabotage {
    None,
    /// Invert every expectation. Every measuring row must go red.
    Want,
    /// Run the handshake but deliver no keystroke. No measuring row may pass.
    NoKeys,
}

/// The modifier keys the OS still believes are held down.
///
/// `NSEvent.modifierFlags` as a class method is the live keyboard state, not a property of some
/// past event, so it catches an injector that pressed a modifier and failed to release it.
fn residual_modifiers() -> String {
    let flags = NSEvent::modifierFlags_class();
    let mut held = Vec::new();
    for (mask, name) in [
        (NSEventModifierFlags::Command, "cmd"),
        (NSEventModifierFlags::Option, "alt"),
        (NSEventModifierFlags::Shift, "shift"),
        (NSEventModifierFlags::Control, "ctrl"),
    ] {
        if flags.contains(mask) {
            held.push(name);
        }
    }
    held.join("+")
}

/// What the OS said about focus while a keystroke was being delivered.
struct Sent {
    key: String,
    /// `unix id of first application process whose frontmost is true`, asked inside the same
    /// AppleScript that sent the keystroke — a pid, because a second copy of this binary answers
    /// to the same name.
    frontmost: String,
    frontmost_ok: bool,
    key_window_before: bool,
    key_window_after: bool,
    /// Modifiers still down afterwards. Any is disqualifying: the next row would be measured with
    /// a chord it never asked for.
    residual_modifiers: String,
}

impl Sent {
    /// Why this row is not a result, if it is not one. A stuck modifier gets its own verdict rather
    /// than sharing `NOKEY`, because the two have different fixes and a stuck modifier corrupts
    /// every row after it as well as this one.
    fn disqualified(&self) -> Option<&'static str> {
        if self.key == "-" {
            return None;
        }
        if !self.residual_modifiers.is_empty() {
            return Some("STUCKMOD");
        }
        if !self.key_window_before || !self.key_window_after || !self.frontmost_ok {
            return Some("NOKEY");
        }
        None
    }

    fn trusted(&self) -> bool {
        self.disqualified().is_none()
    }

    /// The provenance column: which process the OS had in front, whether this window held key on
    /// both sides of the keystroke, and anything still held down after it.
    fn provenance(&self) -> String {
        let mut out = format!("fg={}", self.frontmost);
        if !self.key_window_before || !self.key_window_after {
            out.push_str(&format!(
                " key={}/{}",
                self.key_window_before, self.key_window_after
            ));
        }
        if !self.residual_modifiers.is_empty() {
            out.push_str(&format!(" stuck={}", self.residual_modifiers));
        }
        out
    }
}

/// A view's class plus its placeholder when it has one, which is what distinguishes the two fields.
fn describe(view: Option<&NSView>) -> String {
    let Some(view) = view else {
        return "<nil>".into();
    };
    let class = view.class().name().to_string_lossy().into_owned();
    if !view.isKindOfClass(NSTextField::class()) {
        return class;
    }
    let field = unsafe { Retained::cast_unchecked::<NSTextField>(view.retain()) };
    match field.placeholderString() {
        Some(placeholder) => format!("{class}[{placeholder}]"),
        None => class,
    }
}

struct Row {
    name: &'static str,
    verdict: &'static str,
    pre: String,
    post: String,
    want: String,
    key: String,
    frontmost: String,
}

// ---------------------------------------------------------------------------------------------
// the suite

struct Suite {
    dir: PathBuf,
    seq: u64,
    sabotage: Sabotage,
    handle: WindowHandle<Keys>,
    controls: Controls,
    app: Retained<NSApplication>,
    screen_height: f64,
    rows: Vec<Row>,
    controls_passed: usize,
    /// This process's name, for the report only.
    name: String,
    /// This process's pid, which is what every frontmost test compares against — the name is not
    /// unique when a stale copy of the same binary is still alive.
    pid: u32,
}

async fn sleep(cx: &AsyncApp, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

impl Suite {
    fn window_is_key(&self) -> bool {
        self.controls.window.isKeyWindow() && self.app.isActive()
    }

    /// The responder chain answer, as a label the assertions can compare.
    fn responder(&self) -> String {
        let Some(responder) = self.controls.window.firstResponder() else {
            return "<none>".into();
        };
        for (field, name) in [
            (&self.controls.alpha, "editor:alpha"),
            (&self.controls.beta, "editor:beta"),
        ] {
            if let Some(editor) = field.currentEditor() {
                if same(&*editor, &*responder) {
                    return name.into();
                }
            }
        }
        if same(&*self.controls.toggle, &*responder) {
            return "switch".into();
        }
        if same(&*self.controls.slider, &*responder) {
            return "slider".into();
        }
        if same(&*self.controls.gpui, &*responder) {
            return "gpui".into();
        }
        if same(&*self.controls.window, &*responder) {
            return "window".into();
        }
        responder.class().name().to_string_lossy().into_owned()
    }

    fn selection(&self) -> String {
        match self.controls.alpha.currentEditor() {
            Some(editor) => {
                let range = editor.selectedRange();
                format!("({},{})", range.location, range.length)
            }
            None => "unfocused".into(),
        }
    }

    fn alpha_text(&self) -> String {
        self.controls.alpha.stringValue().to_string()
    }

    /// Text and selection together, with the text escaped so an inserted control character is
    /// visible rather than silently widening the string.
    fn caret_state(&self) -> String {
        format!("{:?}|{}", self.alpha_text(), self.selection())
    }

    /// Put the field back into a known state through GPUI, so the crate's own text sync owns the
    /// value rather than a direct `setStringValue:` the next paint would fight.
    ///
    /// The selection is verified, not just written. `makeFirstResponder:` on an `NSTextField`
    /// selects the whole string, and the field editor is not always installed by the time the next
    /// statement runs — a missed write leaves everything selected, which turns the next Delete into
    /// "clear the field" and reads as a broken key rather than a broken setup.
    async fn arm(&mut self, cx: &mut AsyncApp, text: &str, caret: usize) {
        self.controls.window.makeFirstResponder(None);
        let owned = text.to_owned();
        self.handle
            .update(cx, |keys, _, cx| {
                keys.alpha = owned;
                cx.notify();
            })
            .ok();
        for _ in 0..60 {
            sleep(cx, POLL).await;
            if self.alpha_text() == text {
                break;
            }
        }
        let want = format!("({caret},0)");
        for _ in 0..60 {
            self.controls
                .window
                .makeFirstResponder(Some(&self.controls.alpha));
            if let Some(editor) = self.controls.alpha.currentEditor() {
                editor.setSelectedRange(NSRange {
                    location: caret,
                    length: 0,
                });
            }
            if self.selection() == want {
                return;
            }
            sleep(cx, POLL).await;
        }
    }

    /// Hand one whole AppleScript to `scripts/keys.sh`, which runs it through System Events, and
    /// return what it printed. Distinct temp names: `req` and `req.applescript` would otherwise
    /// both stage through `req.tmp`.
    async fn run_script(&mut self, cx: &mut AsyncApp, script: String) -> String {
        self.seq += 1;
        let scpt = self.dir.join("req.applescript");
        let staged = self.dir.join("staged.applescript");
        fs::write(&staged, script).ok();
        fs::rename(&staged, &scpt).ok();
        let req = self.dir.join("req");
        let staged = self.dir.join("staged.req");
        fs::write(&staged, self.seq.to_string()).ok();
        fs::rename(&staged, &req).ok();

        for _ in 0..800 {
            sleep(cx, POLL).await;
            let acked = fs::read_to_string(self.dir.join("ack"))
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok());
            if acked == Some(self.seq) {
                return fs::read_to_string(self.dir.join("fg"))
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
            }
        }
        "<timeout>".to_owned()
    }

    /// One input action, with the OS asked in the same breath which app was frontmost while it
    /// landed. `body` is an AppleScript fragment for System Events; `tool` is a cliclick argument
    /// list posting raw CGEvents. Exactly one of them carries the action.
    ///
    /// Both tools are here because neither covers the matrix alone. System Events types letters and
    /// letter chords, but its `click at` is a measured no-op: it returned without error and neither
    /// focused a field nor delivered a mouse-down. cliclick posts real CGEvents and its clicks
    /// land, but it cannot press a letter with a modifier held. The `probe.*` rows record what each
    /// event shape actually does to a caret, in a hosted field and in a plain window, rather than
    /// this comment asserting a cause.
    async fn deliver(&mut self, cx: &mut AsyncApp, key: &str, body: &str, tool: &str) -> Sent {
        if !self.window_is_key() {
            cx.update(|cx| cx.activate(true)).ok();
            sleep(cx, Duration::from_millis(300)).await;
        }
        let key_window_before = self.window_is_key();
        let suppress = self.sabotage == Sabotage::NoKeys;
        let delivered = if suppress { "delay 0.01" } else { body };
        // Every modifier is explicitly released after the action, and the residual flags are read
        // back below. Measured: a `ku:shift` went missing once and shift stayed down for the rest of
        // the run, which turned ⌘← into ⇧⌘← — the row still looked like a plausible failure,
        // reporting a (0,3) selection instead of a (0,0) caret, with nothing to say it was the
        // instrument. A stuck modifier has to be loud, not inferred later from a shape.
        let tool = if suppress {
            String::new()
        } else if tool.is_empty() {
            String::new()
        } else {
            format!("{tool} ku:cmd,alt,ctrl,shift,fn")
        };
        fs::write(self.dir.join("req.cliclick"), &tool).ok();
        // The frontmost test and the keystroke are in one script on purpose. Checking from Rust
        // and then sending would leave a window in which this suite types "Hello" into whatever
        // the user had focused instead.
        //
        // The test is on `unix id`, not on the process name. A second copy of this same binary
        // satisfies a name comparison, so a stale instance in front makes every row report a
        // trusted measurement while the keystrokes land in the other window's field. Measured: an
        // entire arm came back with nothing delivered and every row still claiming fg=Keys.
        let pid = self.pid;
        let script = format!(
            "tell application \"System Events\"\n\
             \tset fg to unix id of first application process whose frontmost is true\n\
             \tif fg is {pid} then\n\
             \t\t{delivered}\n\
             \tend if\n\
             end tell\n\
             return (fg as string)\n"
        );
        let frontmost = self.run_script(cx, script).await;
        fs::write(self.dir.join("req.cliclick"), "").ok();
        sleep(cx, SETTLE).await;
        Sent {
            key: key.to_owned(),
            frontmost_ok: frontmost == self.pid.to_string(),
            frontmost,
            key_window_before,
            key_window_after: self.window_is_key(),
            residual_modifiers: residual_modifiers(),
        }
    }

    /// A keystroke System Events can express: a letter, or a letter with modifiers.
    async fn send(&mut self, cx: &mut AsyncApp, key: &str, body: &str) -> Sent {
        self.deliver(cx, key, body, "").await
    }

    /// A keystroke that needs a real CGEvent: anything in the navigation cluster.
    async fn press(&mut self, cx: &mut AsyncApp, key: &str, tool: &str) -> Sent {
        self.deliver(cx, key, "delay 0.02", tool).await
    }

    /// A real mouse click at a screen point.
    ///
    /// The point is checked against this window's own frame first. A click fired at a coordinate
    /// nobody verified lands in whatever app happens to own that pixel.
    async fn click(&mut self, cx: &mut AsyncApp, label: &str, x: f64, y: f64) -> Sent {
        let frame = self.controls.window.frame();
        let top = self.screen_height - (frame.origin.y + frame.size.height);
        let inside = x >= frame.origin.x
            && x <= frame.origin.x + frame.size.width
            && y >= top
            && y <= top + frame.size.height;
        if !inside {
            return Sent {
                key: label.to_owned(),
                frontmost: "<outside own window>".to_owned(),
                frontmost_ok: false,
                key_window_before: self.window_is_key(),
                key_window_after: self.window_is_key(),
                residual_modifiers: residual_modifiers(),
            };
        }
        self.deliver(cx, label, "delay 0.05", &format!("c:{x:.0},{y:.0}"))
            .await
    }

    /// Arm the hosted field, deliver one event shape, and report what it did. A probe, not an
    /// assertion: its job is to tell the difference between an injector that produces the wrong
    /// event and a window that mishandles the right one.
    async fn probe_hosted(&mut self, cx: &mut AsyncApp, tool: &str) -> String {
        self.arm(cx, "abcdef", 3).await;
        let sent = self.press(cx, "probe", tool).await;
        format!("{} trusted={}", self.caret_state(), sent.trusted())
    }

    /// The same event shape delivered to a bare `NSTextField` this suite creates itself, with none
    /// of the crate's configuration on it.
    ///
    /// `parent` chooses where it lives: `"plain"` gives it an `NSWindow` of its own, `"gpui"` makes
    /// it a subview of GPUI's Metal view — exactly where the crate puts hosted controls — and
    /// `"content"` makes it a subview of GPUI's window's content view, a sibling of the Metal view
    /// rather than a child. Together with the crate-hosted rows those four arms separate the
    /// injector from the window from the view from the crate.
    async fn probe_bare_field(&mut self, cx: &mut AsyncApp, tool: &str, parent: &str) -> String {
        let mtm = MainThreadMarker::new().expect("probes run on the main thread");
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(10., 340.), NSSize::new(300., 24.)),
        );
        field.setStringValue(&NSString::from_str("abcdef"));

        // Below the suite's own window, never over it: an overlapping window would change what the
        // click rows land on. `setReleasedWhenClosed(false)` is not optional — a programmatically
        // created NSWindow releases itself on close, and the retained handle here would then be
        // dangling. Measured: without it the process died mid-run with no crash report.
        let frame = self.controls.window.frame();
        let window = match parent {
            "plain" => {
                let window = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        NSWindow::alloc(mtm),
                        NSRect::new(
                            NSPoint::new(frame.origin.x, frame.origin.y - 110.),
                            NSSize::new(320., 60.),
                        ),
                        NSWindowStyleMask::Titled,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                unsafe { window.setReleasedWhenClosed(false) };
                window.setTitle(&NSString::from_str("control"));
                field.setFrame(NSRect::new(NSPoint::new(10., 16.), NSSize::new(300., 24.)));
                if let Some(content) = window.contentView() {
                    content.addSubview(&field);
                }
                window.makeKeyAndOrderFront(None);
                Some(window)
            }
            "content" => {
                if let Some(content) = self.controls.window.contentView() {
                    content.addSubview(&field);
                }
                None
            }
            _ => {
                self.controls.gpui.addSubview(&field);
                None
            }
        };
        let host = window
            .clone()
            .unwrap_or_else(|| self.controls.window.clone());

        let mut armed = "<never armed>".to_owned();
        for _ in 0..60 {
            host.makeFirstResponder(Some(&field));
            if let Some(editor) = field.currentEditor() {
                editor.setSelectedRange(NSRange {
                    location: 3,
                    length: 0,
                });
                let range = editor.selectedRange();
                armed = format!("({},{})", range.location, range.length);
                if armed == "(3,0)" {
                    break;
                }
            }
            sleep(cx, POLL).await;
        }
        let key = host.isKeyWindow();
        let pid = self.pid;
        let script = format!(
            "tell application \"System Events\"\n\
             \tset fg to unix id of first application process whose frontmost is true\n\
             \tif fg is {pid} then\n\
             \t\tdelay 0.02\n\
             \tend if\n\
             end tell\n\
             return (fg as string)\n"
        );
        fs::write(self.dir.join("req.cliclick"), tool).ok();
        let frontmost = self.run_script(cx, script).await;
        fs::write(self.dir.join("req.cliclick"), "").ok();
        sleep(cx, SETTLE).await;

        let editor_state = match field.currentEditor() {
            Some(editor) => {
                let range = editor.selectedRange();
                format!("({},{})", range.location, range.length)
            }
            None => "unfocused".into(),
        };
        let result = format!(
            "{:?}|{editor_state} armed={armed} host_key={key} fg={frontmost}",
            field.stringValue().to_string()
        );
        host.makeFirstResponder(None);
        field.removeFromSuperview();
        if let Some(window) = window {
            window.orderOut(None);
        }
        self.controls.window.makeKeyAndOrderFront(None);
        sleep(cx, Duration::from_millis(200)).await;
        result
    }

    /// The menu bar as the OS sees it, which is the fact the ⌘ keys hang off and the one thing this
    /// process could be wrong about from the inside.
    async fn menu_bar_from_os(&mut self, cx: &mut AsyncApp) -> String {
        let pid = self.pid;
        let script = format!(
            "tell application \"System Events\"\n\
             \tset self to first application process whose unix id is {pid}\n\
             \tset fg to unix id of first application process whose frontmost is true\n\
             \ttry\n\
             \t\tset bar to name of every menu bar item of menu bar 1 of self\n\
             \ton error message\n\
             \t\tset bar to {{message}}\n\
             \tend try\n\
             end tell\n\
             return ((name of self) & \" frontmost_pid=\" & (fg as string) & \" :: \" & (bar as string))\n"
        );
        self.run_script(cx, script).await
    }

    fn push(&mut self, name: &'static str, verdict: &'static str, row: Row) {
        let row = Row {
            name,
            verdict,
            ..row
        };
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.name, row.verdict, row.pre, row.post, row.want, row.key, row.frontmost
        );
        println!("{line}");
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("results.tsv"))
        {
            let _ = writeln!(file, "{line}");
        }
        self.rows.push(row);
    }

    fn expect(&mut self, name: &'static str, pre: &str, post: &str, want: &str, sent: &Sent) {
        let want = match self.sabotage {
            Sabotage::Want => format!("{want}·sabotaged"),
            _ => want.to_owned(),
        };
        let verdict = if let Some(reason) = sent.disqualified() {
            reason
        } else if post != want {
            "FAIL"
        } else if pre == want {
            "VACUOUS"
        } else {
            "PASS"
        };
        self.push(
            name,
            verdict,
            Row {
                name,
                verdict,
                pre: pre.to_owned(),
                post: post.to_owned(),
                want,
                key: sent.key.clone(),
                frontmost: sent.provenance(),
            },
        );
    }

    /// For a row whose expectation is only "this moved", because the step size is AppKit's to
    /// choose and hardcoding it would test this file rather than the system.
    fn expect_change(&mut self, name: &'static str, pre: &str, post: &str, sent: &Sent) {
        let inverted = self.sabotage == Sabotage::Want;
        let changed = pre != post;
        let verdict = if let Some(reason) = sent.disqualified() {
            reason
        } else if changed != !inverted {
            "FAIL"
        } else {
            "PASS"
        };
        self.push(
            name,
            verdict,
            Row {
                name,
                verdict,
                pre: pre.to_owned(),
                post: post.to_owned(),
                want: if inverted { "= pre" } else { "≠ pre" }.into(),
                key: sent.key.clone(),
                frontmost: sent.provenance(),
            },
        );
    }

    /// The negative control: a chord bound to nothing must move nothing. Passes under
    /// `KEYS_SABOTAGE=nokeys` by construction, and is excluded from that arm's claim.
    fn expect_stable(&mut self, name: &'static str, pre: &str, post: &str, sent: &Sent) {
        let inverted = self.sabotage == Sabotage::Want;
        let stable = pre == post;
        let verdict = if let Some(reason) = sent.disqualified() {
            reason
        } else if stable != !inverted {
            "FAIL"
        } else {
            "PASS"
        };
        if verdict == "PASS" {
            self.controls_passed += 1;
        }
        self.push(
            name,
            verdict,
            Row {
                name,
                verdict,
                pre: pre.to_owned(),
                post: post.to_owned(),
                want: if inverted { "≠ pre" } else { "= pre" }.into(),
                key: sent.key.clone(),
                frontmost: sent.provenance(),
            },
        );
    }

    fn note(&mut self, name: &'static str, value: String) {
        self.push(
            name,
            "NOTE",
            Row {
                name,
                verdict: "NOTE",
                pre: String::new(),
                post: value,
                want: String::new(),
                key: "-".into(),
                frontmost: String::new(),
            },
        );
    }

    /// Screen point, in the top-left-origin space System Events clicks in, at the centre of a view.
    fn point_of(&self, view: &NSView) -> (f64, f64) {
        let in_window = view.convertRect_toView(view.bounds(), None);
        let on_screen = self.controls.window.convertRectToScreen(in_window);
        (
            on_screen.origin.x + on_screen.size.width / 2.,
            self.screen_height - (on_screen.origin.y + on_screen.size.height / 2.),
        )
    }

    /// A point in the right-hand column, which holds no hosted view, at mid height so it misses
    /// the titlebar band a full-size content view draws under.
    fn gpui_point(&self) -> (f64, f64) {
        let frame = self.controls.window.frame();
        (
            frame.origin.x + frame.size.width - 80.,
            self.screen_height - (frame.origin.y + frame.size.height / 2.),
        )
    }

    async fn model(&mut self, cx: &mut AsyncApp) -> (u32, u32, u32, bool) {
        self.handle
            .update(cx, |keys, window, _| {
                (
                    keys.default_clicks,
                    keys.submits,
                    keys.gpui_clicks,
                    keys.focus.is_focused(window),
                )
            })
            .unwrap_or((0, 0, 0, false))
    }
}

fn menu_titles(app: &NSApplication) -> String {
    match app.mainMenu() {
        None => "<no main menu>".into(),
        Some(menu) => {
            let mut titles = Vec::new();
            for index in 0..menu.numberOfItems() {
                if let Some(item) = menu.itemAtIndex(index) {
                    titles.push(item.title().to_string());
                }
            }
            if titles.is_empty() {
                "<empty main menu>".into()
            } else {
                titles.join("/")
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the matrix

async fn drive(cx: &mut AsyncApp, handle: WindowHandle<Keys>, dir: PathBuf) {
    let mtm = MainThreadMarker::new().expect("gpui runs its app closure on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // The flip between AppKit's bottom-left global space and the top-left space a click tool uses
    // is the height of the display at the origin, which is not necessarily `screens[0]` — this
    // machine has a second display at negative y, so guessing costs a click in the wrong window.
    let screens = NSScreen::screens(mtm);
    let mut screen_height = 0.;
    for index in 0..screens.count() {
        let frame = screens.objectAtIndex(index).frame();
        if frame.origin.x == 0. && frame.origin.y == 0. {
            screen_height = frame.size.height;
        }
    }
    if screen_height == 0. && screens.count() > 0 {
        screen_height = screens.objectAtIndex(0).frame().size.height;
    }

    let mut controls = None;
    for _ in 0..200 {
        sleep(cx, POLL).await;
        controls = handle
            .update(cx, |_, window, _| Controls::find(window))
            .ok()
            .flatten();
        if controls.is_some() {
            break;
        }
    }
    let Some(controls) = controls else {
        fs::write(dir.join("results.tsv"), "fatal\tno hosted views found\n").ok();
        fs::write(dir.join("done"), "1").ok();
        cx.update(|cx| cx.quit()).ok();
        return;
    };

    let sabotage = match std::env::var("KEYS_SABOTAGE").as_deref() {
        Ok("want") => Sabotage::Want,
        Ok("nokeys") => Sabotage::NoKeys,
        _ => Sabotage::None,
    };
    let mut suite = Suite {
        dir: dir.clone(),
        seq: 0,
        sabotage,
        handle,
        controls,
        app,
        screen_height,
        rows: Vec::new(),
        controls_passed: 0,
        name: NSProcessInfo::processInfo().processName().to_string(),
        pid: std::process::id(),
    };
    // The driver re-checks who is frontmost immediately before a click, and this is what it
    // compares against: a pid, so a stale copy of this same binary cannot satisfy the test.
    fs::write(dir.join("who"), suite.pid.to_string()).ok();
    fs::write(dir.join("req.cliclick"), "").ok();

    for _ in 0..400 {
        if suite.window_is_key() {
            break;
        }
        sleep(cx, POLL).await;
    }

    let saved_board = board();
    let menus = menu_titles(&suite.app);
    let full_keyboard_access = suite.app.isFullKeyboardAccessEnabled();
    suite.note("env.menus", menus);
    suite.note("env.full_keyboard_access", full_keyboard_access.to_string());
    suite.note(
        "env.install_menus",
        match std::env::var("KEYS_MENUS").as_deref() {
            Ok("0") => "skipped".into(),
            _ => "called".into(),
        },
    );
    suite.note(
        "env.sabotage",
        match sabotage {
            Sabotage::None => "none".into(),
            Sabotage::Want => "want".into(),
            Sabotage::NoKeys => "nokeys".into(),
        },
    );
    suite.note("env.window_is_key", suite.window_is_key().to_string());
    suite.note("env.process", format!("{} pid={}", suite.name, suite.pid));
    suite.note("env.screen_height", format!("{screen_height:.0}"));
    let frame = suite.controls.window.frame();
    suite.note(
        "env.window_frame",
        format!(
            "x={:.0} y={:.0} w={:.0} h={:.0}",
            frame.origin.x, frame.origin.y, frame.size.width, frame.size.height
        ),
    );
    let (ax, ay) = suite.point_of(&suite.controls.alpha);
    let (gx, gy) = suite.gpui_point();
    suite.note(
        "env.click_points",
        format!("alpha=({ax:.0},{ay:.0}) gpui=({gx:.0},{gy:.0})"),
    );
    suite.note(
        "env.default_button_cell",
        suite
            .controls
            .window
            .defaultButtonCell()
            .is_some()
            .to_string(),
    );
    let os_menu_bar = suite.menu_bar_from_os(cx).await;
    suite.note("env.menu_bar_from_os", os_menu_bar);

    // -- focus by a real click ------------------------------------------------------------------
    suite.arm(cx, "abcdef", 6).await;
    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let (x, y) = suite.point_of(&suite.controls.alpha);
    let sent = suite.click(cx, "click alpha", x, y).await;
    let post = suite.responder();
    suite.expect("focus.click_field", &pre, &post, "editor:alpha", &sent);

    // -- plain and non-ASCII typing --------------------------------------------------------------
    suite.arm(cx, "", 0).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "H e l l o", "keystroke \"Hello\"").await;
    let post = suite.alpha_text();
    suite.expect("type.plain", &pre, &post, "Hello", &sent);

    // Measured: `keystroke (character id 233)` lands a bare "a", so the non-ASCII row goes through
    // the real dead-key path instead — ⌥e arms the acute accent, then "e" commits "é". That is
    // also the more honest test, since it exercises the input method rather than a shortcut.
    suite.arm(cx, "Hello", 5).await;
    let pre = suite.alpha_text();
    let sent = suite
        .send(
            cx,
            "opt-e e",
            "key code 14 using {option down}\nkeystroke \"e\"",
        )
        .await;
    let post = suite.alpha_text();
    suite.expect("type.non_ascii", &pre, &post, "Hello\u{e9}", &sent);

    suite.arm(cx, "Hello", 5).await;
    let sent = suite
        .send(cx, "char-id-233", "keystroke (character id 233)")
        .await;
    let landed = suite.alpha_text();
    let _ = sent;
    suite.note("type.char_id_233_lands", format!("{landed:?}"));

    // -- instrument probes -----------------------------------------------------------------------
    //
    // A bare injected arrow key inserts U+001C into the hosted field instead of moving the caret.
    // These rows establish whether that is the injector producing the wrong event or the window
    // mishandling the right one: same field with a function-flagged arrow, same field with System
    // Events' own arrow, and the same events into a plain NSTextField in a plain NSWindow.
    let state = suite.probe_hosted(cx, "kp:arrow-left").await;
    suite.note("probe.hosted_arrow_bare", state);
    let state = suite.probe_hosted(cx, "kd:fn kp:arrow-left ku:fn").await;
    suite.note("probe.hosted_arrow_fn", state);
    suite.arm(cx, "abcdef", 3).await;
    let sent = suite.send(cx, "left(se)", "key code 123").await;
    let state = format!("{} trusted={}", suite.caret_state(), sent.trusted());
    suite.note("probe.hosted_arrow_system_events", state);
    let state = suite.probe_bare_field(cx, "kp:arrow-left", "plain").await;
    suite.note("probe.bare_field_plain_window", state);
    let state = suite.probe_bare_field(cx, "kp:arrow-left", "content").await;
    suite.note("probe.bare_field_window_content_view", state);
    let state = suite.probe_bare_field(cx, "kp:arrow-left", "gpui").await;
    suite.note("probe.bare_field_gpui_view", state);

    // Home and End in a window this suite fully controls, so the caret rows below have something
    // to be measured against. Measured: a plain `NSWindow`'s field editor answers Home with
    // `scrollToBeginningOfDocument:` and End with `scrollToEndOfDocument:`, neither of which moves
    // the insertion point in a single-line editor — the caret stays where it was armed.
    for (name, tool) in [
        ("probe.bare_field_plain_home", "kp:home"),
        ("probe.bare_field_plain_end", "kp:end"),
    ] {
        let state = suite.probe_bare_field(cx, tool, "plain").await;
        suite.note(name, state);
    }

    for (name, tool) in [
        ("probe.hosted_space", "kp:space"),
        ("probe.hosted_esc", "kp:esc"),
        ("probe.hosted_delete", "kp:delete"),
        ("probe.hosted_fwd_delete", "kp:fwd-delete"),
        ("probe.hosted_fwd_delete_fn", "kd:fn kp:fwd-delete ku:fn"),
        ("probe.hosted_home_fn", "kd:fn kp:home ku:fn"),
        ("probe.hosted_tab", "kp:tab"),
    ] {
        let state = suite.probe_hosted(cx, tool).await;
        suite.note(name, state);
    }

    let (alpha_next, beta_next) = unsafe {
        (
            suite.controls.alpha.nextKeyView(),
            suite.controls.beta.nextKeyView(),
        )
    };
    suite.note(
        "probe.key_view_loop",
        format!(
            "autorecalculates={} alpha.next={} beta.next={}",
            suite.controls.window.autorecalculatesKeyViewLoop(),
            describe(alpha_next.as_deref()),
            describe(beta_next.as_deref()),
        ),
    );

    // Membership in a key view loop is `canBecomeKeyView`, not `acceptsFirstResponder` and not
    // `makeFirstResponder:` — which is why a view can be focused programmatically and still be
    // invisible to Tab. Without this note a chain that skips a link reads as a chain that was
    // never written.
    let alpha_prev = unsafe { suite.controls.alpha.previousKeyView() };
    suite.note(
        "probe.can_become_key_view",
        format!(
            "gpui={} alpha={} switch={} slider={} alpha.prev={} prev_keyable={}",
            suite.controls.gpui.canBecomeKeyView(),
            suite.controls.alpha.canBecomeKeyView(),
            suite.controls.toggle.canBecomeKeyView(),
            suite.controls.slider.canBecomeKeyView(),
            describe(alpha_prev.as_deref()),
            alpha_prev
                .as_deref()
                .is_some_and(|view| view.canBecomeKeyView()),
        ),
    );

    // -- caret and selection ---------------------------------------------------------------------
    //
    // Every caret row asserts the text too. A row that checked only `selectedRange` would read an
    // inserted character as a caret move — both raise the location by one — and the first run of
    // this suite did exactly that, scoring a right-arrow green on an insertion.
    for (name, text, caret, key, tool, sel) in [
        (
            "caret.left",
            "abcdef",
            6usize,
            "left",
            "kp:arrow-left",
            "(5,0)",
        ),
        (
            "caret.right",
            "abcdef",
            2,
            "right",
            "kp:arrow-right",
            "(3,0)",
        ),
        (
            "caret.shift_left",
            "abcdef",
            4,
            "shift-left",
            "kd:shift kp:arrow-left ku:shift",
            "(3,1)",
        ),
        (
            "caret.shift_right",
            "abcdef",
            2,
            "shift-right",
            "kd:shift kp:arrow-right ku:shift",
            "(2,1)",
        ),
        (
            "caret.option_left_word",
            "alpha beta gamma",
            16,
            "opt-left",
            "kd:alt kp:arrow-left ku:alt",
            "(11,0)",
        ),
        (
            "caret.option_right_word",
            "alpha beta gamma",
            0,
            "opt-right",
            "kd:alt kp:arrow-right ku:alt",
            "(5,0)",
        ),
        (
            "caret.cmd_left_line_start",
            "abcdef",
            3,
            "cmd-left",
            "kd:cmd kp:arrow-left ku:cmd",
            "(0,0)",
        ),
        (
            "caret.cmd_right_line_end",
            "abcdef",
            3,
            "cmd-right",
            "kd:cmd kp:arrow-right ku:cmd",
            "(6,0)",
        ),
    ] {
        suite.arm(cx, text, caret).await;
        let pre = suite.caret_state();
        let sent = suite.press(cx, key, tool).await;
        let post = suite.caret_state();
        suite.expect(name, &pre, &post, &format!("{text:?}|{sel}"), &sent);
    }

    // Home and End are asserted as no-ops on the caret, because that is what a field editor does
    // with them: it answers Home with `scrollToBeginningOfDocument:` and End with
    // `scrollToEndOfDocument:` — observed on the editor's own `doCommandBySelector:` — and neither
    // touches the insertion point. Measured against an unhosted control, not assumed: the
    // `probe.bare_field_plain_*` rows deliver the same two keys to a bare `NSTextField` in a plain
    // `NSWindow` and the caret stays put there too. Matching an unhosted field is this crate's
    // contract, so what these rows catch is the failure that was real — the key arriving as an
    // inserted U+0001 or U+0004.
    for (name, key, tool) in [
        ("caret.home", "home", "kp:home"),
        ("caret.end", "end", "kp:end"),
    ] {
        suite.arm(cx, "abcdef", 3).await;
        let pre = suite.caret_state();
        let sent = suite.press(cx, key, tool).await;
        let post = suite.caret_state();
        suite.expect_stable(name, &pre, &post, &sent);
    }

    // Text and selection again: a Delete that clears the whole field means the setup left
    // everything selected, which is a broken `arm`, not a broken key — and a text-only row cannot
    // show the difference.
    for (name, text, caret, key, tool, want) in [
        (
            "edit.backspace",
            "abcdef",
            6usize,
            "delete",
            "kp:delete",
            "\"abcde\"|(5,0)",
        ),
        (
            "edit.forward_delete",
            "abcdef",
            3,
            "fwd-delete",
            "kp:fwd-delete",
            "\"abcef\"|(3,0)",
        ),
    ] {
        suite.arm(cx, text, caret).await;
        let pre = suite.caret_state();
        let sent = suite.press(cx, key, tool).await;
        let post = suite.caret_state();
        suite.expect(name, &pre, &post, want, &sent);
    }

    // -- the Edit-menu commands ------------------------------------------------------------------
    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.selection();
    let sent = suite
        .send(cx, "cmd-a", "keystroke \"a\" using {command down}")
        .await;
    let post = suite.selection();
    suite.expect("edit.cmd_a_select_all", &pre, &post, "(0,6)", &sent);

    // Copy is isolated from select-all: the selection is set here, not typed, so a red ⌘A cannot
    // make ⌘C look broken.
    suite.arm(cx, "abcdef", 0).await;
    if let Some(editor) = suite.controls.alpha.currentEditor() {
        editor.setSelectedRange(NSRange {
            location: 0,
            length: 6,
        });
    }
    set_board("sentinel-copy");
    let pre = board();
    let sent = suite
        .send(cx, "cmd-c", "keystroke \"c\" using {command down}")
        .await;
    let post = board();
    suite.expect("edit.cmd_c_copy", &pre, &post, "abcdef", &sent);

    suite.arm(cx, "abcdef", 6).await;
    set_board("XY");
    let pre = suite.alpha_text();
    let sent = suite
        .send(cx, "cmd-v", "keystroke \"v\" using {command down}")
        .await;
    let post = suite.alpha_text();
    suite.expect("edit.cmd_v_paste", &pre, &post, "abcdefXY", &sent);

    suite.arm(cx, "abcdef", 0).await;
    if let Some(editor) = suite.controls.alpha.currentEditor() {
        editor.setSelectedRange(NSRange {
            location: 0,
            length: 6,
        });
    }
    set_board("sentinel-cut");
    let pre = format!("text={}|board={}", suite.alpha_text(), board());
    let sent = suite
        .send(cx, "cmd-x", "keystroke \"x\" using {command down}")
        .await;
    let post = format!("text={}|board={}", suite.alpha_text(), board());
    suite.expect("edit.cmd_x_cut", &pre, &post, "text=|board=abcdef", &sent);

    // Undo needs a real edit to undo, so the setup keystroke is asserted in its own right.
    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "X", "keystroke \"X\"").await;
    let typed = suite.alpha_text();
    suite.expect("edit.undo_setup_typed", &pre, &typed, "abcdefX", &sent);
    let sent = suite
        .send(cx, "cmd-z", "keystroke \"z\" using {command down}")
        .await;
    let post = suite.alpha_text();
    suite.expect("edit.cmd_z_undo", &typed, &post, "abcdef", &sent);

    // Redo is isolated from ⌘Z the same way copy is from select-all: the undo it has to reverse is
    // performed through the undo manager, not through the key under test.
    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "X", "keystroke \"X\"").await;
    let typed = suite.alpha_text();
    suite.expect("edit.redo_setup_typed", &pre, &typed, "abcdefX", &sent);
    let undone = match suite
        .controls
        .alpha
        .currentEditor()
        .and_then(|editor| editor.undoManager())
    {
        Some(manager) => {
            manager.undo();
            suite.alpha_text()
        }
        None => "<no undo manager>".into(),
    };
    suite.note(
        "edit.redo_setup",
        format!("typed={typed}|programmatic_undo={undone}"),
    );
    let sent = suite
        .send(
            cx,
            "shift-cmd-z",
            "keystroke \"z\" using {command down, shift down}",
        )
        .await;
    let post = suite.alpha_text();
    suite.expect("edit.shift_cmd_z_redo", &undone, &post, "abcdefX", &sent);

    // -- focus movement --------------------------------------------------------------------------
    //
    // The text rides along in these rows because a Tab that got inserted as U+0009 rather than
    // moving focus leaves the responder unchanged, and a responder-only row cannot tell the two
    // apart in its output.
    let focus_state =
        |suite: &Suite| format!("resp={}|alpha={:?}", suite.responder(), suite.alpha_text());

    suite.arm(cx, "abcdef", 6).await;
    let pre = focus_state(&suite);
    let sent = suite.press(cx, "tab", "kp:tab").await;
    let post = focus_state(&suite);
    suite.expect(
        "focus.tab_next",
        &pre,
        &post,
        "resp=editor:beta|alpha=\"abcdef\"",
        &sent,
    );

    suite.arm(cx, "abcdef", 6).await;
    suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.beta));
    sleep(cx, POLL).await;
    let pre = focus_state(&suite);
    let sent = suite
        .press(cx, "shift-tab", "kd:shift kp:tab ku:shift")
        .await;
    let post = focus_state(&suite);
    suite.expect(
        "focus.shift_tab_prev",
        &pre,
        &post,
        "resp=editor:alpha|alpha=\"abcdef\"",
        &sent,
    );

    suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.beta));
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let sent = suite.press(cx, "tab", "kp:tab").await;
    let post = suite.responder();
    suite.expect_change("focus.tab_leaves_last_field", &pre, &post, &sent);

    // The mirror of the row above, and the only one that pins down what the ends of the loop are
    // for. `expect_change` would pass on any neighbour; this names GPUI's view, so a chain closed
    // into a ring among the hosted views — which would make Tab unable to ever leave them — fails
    // here instead of looking correct.
    suite.arm(cx, "abcdef", 0).await;
    suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.alpha));
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let sent = suite
        .press(cx, "shift-tab", "kd:shift kp:tab ku:shift")
        .await;
    let post = suite.responder();
    suite.expect(
        "focus.shift_tab_leaves_first_field",
        &pre,
        &post,
        "gpui",
        &sent,
    );

    suite.arm(cx, "abcdef", 6).await;
    let (_, _, clicks_before, _) = suite.model(cx).await;
    let pre = format!("resp={}|gpui_clicks={}", suite.responder(), clicks_before);
    let (x, y) = suite.gpui_point();
    let sent = suite.click(cx, "click gpui", x, y).await;
    let (_, _, clicks_after, gpui_focused) = suite.model(cx).await;
    let post = format!("resp={}|gpui_clicks={}", suite.responder(), clicks_after);
    suite.expect(
        "focus.click_gpui_returns_responder",
        &pre,
        &post,
        &format!("resp=gpui|gpui_clicks={}", clicks_before + 1),
        &sent,
    );
    suite.note("focus.gpui_handle_focused", gpui_focused.to_string());

    // -- the non-text controls -------------------------------------------------------------------
    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let took = suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.toggle));
    sleep(cx, POLL).await;
    let post = suite.responder();
    let no_key = Sent {
        key: "-".into(),
        frontmost: String::new(),
        frontmost_ok: true,
        key_window_before: true,
        key_window_after: true,
        residual_modifiers: String::new(),
    };
    suite.note("control.switch_makeFirstResponder", took.to_string());
    suite.expect("control.switch_takes_focus", &pre, &post, "switch", &no_key);

    // `want` is derived from the state read *before* the keystroke. Derived from the state after it
    // — the same live read `post` comes from — it is `!post`, which no behaviour can ever equal, so
    // the row reports FAIL whether the switch toggled or not. This one had that shape and stayed
    // red across two opposite behaviours: `pre[false] post[false] want[true]` while the keystroke
    // was being dropped, then `pre[false] post[true] want[false]` once it was not. Inverting every
    // expectation under `KEYS_SABOTAGE=want` cannot catch it either, because `!post·sabotaged`
    // never equals `post` for the same reason.
    let on = |suite: &Suite| suite.controls.toggle.state() == NSControlStateValueOn;
    let before = on(&suite);
    let pre = before.to_string();
    let sent = suite.press(cx, "space", "kp:space").await;
    let post = on(&suite).to_string();
    suite.expect(
        "control.space_toggles_switch",
        &pre,
        &post,
        &(!before).to_string(),
        &sent,
    );

    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let took = suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.slider));
    sleep(cx, POLL).await;
    let post = suite.responder();
    let no_key = Sent {
        key: "-".into(),
        frontmost: String::new(),
        frontmost_ok: true,
        key_window_before: true,
        key_window_after: true,
        residual_modifiers: String::new(),
    };
    suite.note("control.slider_makeFirstResponder", took.to_string());
    suite.expect("control.slider_takes_focus", &pre, &post, "slider", &no_key);

    let pre = format!("{:.4}", suite.controls.slider.doubleValue());
    let sent = suite.press(cx, "right", "kp:arrow-right").await;
    let post = format!("{:.4}", suite.controls.slider.doubleValue());
    suite.expect_change("control.arrow_moves_slider", &pre, &post, &sent);

    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let (default_before, _, _, _) = suite.model(cx).await;
    let pre = default_before.to_string();
    let sent = suite.press(cx, "return", "kp:return").await;
    let (default_after, _, _, _) = suite.model(cx).await;
    suite.expect(
        "control.return_fires_default_button",
        &pre,
        &default_after.to_string(),
        &(default_before + 1).to_string(),
        &sent,
    );

    suite.arm(cx, "abcdef", 6).await;
    let (_, submits_before, _, _) = suite.model(cx).await;
    let pre = submits_before.to_string();
    let sent = suite.press(cx, "return", "kp:return").await;
    let (_, submits_after, _, _) = suite.model(cx).await;
    suite.expect(
        "control.return_in_field_submits",
        &pre,
        &submits_after.to_string(),
        &(submits_before + 1).to_string(),
        &sent,
    );

    // Escape reaches the field editor's `cancelOperation:` and leaves the edit in place. It does
    // *not* revert: measured with a bare `NSTextField` in a plain `NSWindow`, typing a character
    // and then pressing Escape kept the typed character, so reverting is something a field's owner
    // implements rather than something AppKit does. The row asserts the measured contract — a
    // hosted field answers Escape the way an unhosted one does — and the regression it catches is
    // Escape arriving as an inserted U+001B.
    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "Z", "keystroke \"Z\"").await;
    let typed = suite.alpha_text();
    suite.expect("control.escape_setup_typed", &pre, &typed, "abcdefZ", &sent);
    let sent = suite.press(cx, "escape", "kp:esc").await;
    let post = suite.alpha_text();
    suite.expect_stable("control.escape_leaves_field_alone", &typed, &post, &sent);

    // -- negative control -----------------------------------------------------------------------
    suite.arm(cx, "abcdef", 3).await;
    set_board("sentinel-control");
    let state = |suite: &Suite| {
        format!(
            "text={}|sel={}|board={}|resp={}",
            suite.alpha_text(),
            suite.selection(),
            board(),
            suite.responder()
        )
    };
    let pre = state(&suite);
    let sent = suite
        .send(
            cx,
            "cmd-opt-shift-k",
            "key code 40 using {command down, option down, shift down}",
        )
        .await;
    let post = state(&suite);
    suite.expect_stable("control.unbound_chord_changes_nothing", &pre, &post, &sent);

    // -- summary ---------------------------------------------------------------------------------
    set_board(&saved_board);
    let mut counts = [0usize; 6];
    for row in &suite.rows {
        let slot = match row.verdict {
            "PASS" => 0,
            "FAIL" => 1,
            "VACUOUS" => 2,
            "NOKEY" => 3,
            "STUCKMOD" => 4,
            _ => 5,
        };
        counts[slot] += 1;
    }
    suite.note(
        "summary",
        format!(
            "pass={} fail={} vacuous={} nokey={} stuckmod={} notes={} control_rows_passing={}",
            counts[0], counts[1], counts[2], counts[3], counts[4], counts[5], suite.controls_passed
        ),
    );

    fs::write(dir.join("done"), "1").ok();
    sleep(cx, Duration::from_millis(120)).await;
    cx.update(|cx| cx.quit()).ok();
}

fn main() {
    let dir = PathBuf::from(std::env::var("KEYS_DIR").unwrap_or_else(|_| "/tmp/ng-keys".into()));
    fs::create_dir_all(&dir).ok();

    Application::new().run(move |cx: &mut App| {
        if std::env::var("KEYS_MENUS").as_deref() != Ok("0") {
            native_gpui::install_menus(cx);
        }
        let handle = cx
            .open_window(
                gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(180.), px(180.)),
                        size: size(px(760.), px(420.)),
                    })),
                    ..window_options()
                },
                |_, cx| {
                    cx.new(|cx| Keys {
                        alpha: "abcdef".into(),
                        beta: String::new(),
                        toggle: false,
                        slider: 50.,
                        default_clicks: 0,
                        plain_clicks: 0,
                        submits: 0,
                        gpui_clicks: 0,
                        focus: cx.focus_handle(),
                    })
                },
            )
            .expect("the suite's own window always opens");
        cx.activate(true);
        cx.spawn(async move |cx: &mut AsyncApp| drive(cx, handle, dir).await)
            .detach();
    });
}
