use std::fmt::Display;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Bounds, Context, FocusHandle, MouseDownEvent, Render, Window,
    WindowBounds, WindowHandle, div, point, prelude::*, px, rgb, size,
};
use native_gpui::{
    native_button, native_root, native_slider, native_switch, native_text_field, window_options,
};
use objc2::rc::Retained;
use objc2::{DowncastTarget, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSPasteboard, NSPasteboardTypeString, NSResponder, NSScreen, NSSlider, NSSwitch, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSPoint, NSProcessInfo, NSRange, NSRect, NSSize, NSString,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const SETTLE: Duration = Duration::from_millis(160);
const POLL: Duration = Duration::from_millis(15);

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
                        .child(format!(
                            "gpui clicks {} · default {} · plain {} · submits {}",
                            self.gpui_clicks, self.default_clicks, self.plain_clicks, self.submits
                        )),
                ),
        )
    }
}

struct Controls {
    window: Retained<NSWindow>,
    gpui: Retained<NSView>,
    alpha: Retained<NSTextField>,
    beta: Retained<NSTextField>,
    toggle: Retained<NSSwitch>,
    slider: Retained<NSSlider>,
}

fn descend(view: &NSView, out: &mut Vec<Retained<NSView>>) {
    for sub in view.subviews().iter() {
        descend(&sub, out);
        out.push(sub);
    }
}

fn as_kind<T: DowncastTarget>(view: &Retained<NSView>) -> Option<Retained<T>> {
    view.downcast_ref::<T>().map(T::retain)
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
            toggle: all.iter().find_map(as_kind::<NSSwitch>)?,
            slider: all.iter().find_map(as_kind::<NSSlider>)?,
            window: ns_window,
            gpui,
        })
    }
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

fn selection_of(field: &NSTextField) -> String {
    match field.currentEditor() {
        Some(editor) => {
            let range = editor.selectedRange();
            format!("({},{})", range.location, range.length)
        }
        None => "unfocused".into(),
    }
}

/// `makeFirstResponder:` selects everything and may not install the editor yet: read it back.
async fn focus_at(cx: &AsyncApp, window: &NSWindow, field: &NSTextField, caret: usize) -> String {
    let want = format!("({caret},0)");
    let mut armed = "<never armed>".to_owned();
    for _ in 0..60 {
        window.makeFirstResponder(Some(field));
        if let Some(editor) = field.currentEditor() {
            editor.setSelectedRange(NSRange {
                location: caret,
                length: 0,
            });
            armed = selection_of(field);
            if armed == want {
                break;
            }
        }
        sleep(cx, POLL).await;
    }
    armed
}

#[derive(PartialEq)]
enum Sabotage {
    None,
    Want,
    NoKeys,
}

/// The class method is live keyboard state, so it catches an injector that never let go.
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

struct Sent {
    key: &'static str,
    frontmost: String,
    frontmost_ok: bool,
    key_window_before: bool,
    key_window_after: bool,
    residual_modifiers: String,
}

impl Sent {
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

fn describe(view: Option<&NSView>) -> String {
    let Some(view) = view else {
        return "<nil>".into();
    };
    let class = view.class().name().to_string_lossy();
    match view
        .downcast_ref::<NSTextField>()
        .and_then(|field| field.placeholderString())
    {
        Some(placeholder) => format!("{class}[{placeholder}]"),
        None => class.into_owned(),
    }
}

struct Suite {
    dir: PathBuf,
    seq: u64,
    sabotage: Sabotage,
    handle: WindowHandle<Keys>,
    controls: Controls,
    app: Retained<NSApplication>,
    screen_height: f64,
    tally: [usize; 6],
    controls_passed: usize,
    pid: u32,
}

async fn sleep(cx: &AsyncApp, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

impl Suite {
    fn window_is_key(&self) -> bool {
        self.controls.window.isKeyWindow() && self.app.isActive()
    }

    fn responder(&self) -> String {
        let Some(responder) = self.controls.window.firstResponder() else {
            return "<none>".into();
        };
        let is = |candidate: &NSResponder| std::ptr::eq(candidate, &*responder);
        let editing = |field: &NSTextField| field.currentEditor().is_some_and(|editor| is(&editor));
        let name = if editing(&self.controls.alpha) {
            "editor:alpha"
        } else if editing(&self.controls.beta) {
            "editor:beta"
        } else if is(&self.controls.toggle) {
            "switch"
        } else if is(&self.controls.slider) {
            "slider"
        } else if is(&self.controls.gpui) {
            "gpui"
        } else if is(&self.controls.window) {
            "window"
        } else {
            return responder.class().name().to_string_lossy().into_owned();
        };
        name.into()
    }

    fn selection(&self) -> String {
        selection_of(&self.controls.alpha)
    }

    fn select(&self, location: usize, length: usize) {
        if let Some(editor) = self.controls.alpha.currentEditor() {
            editor.setSelectedRange(NSRange { location, length });
        }
    }

    fn alpha_text(&self) -> String {
        self.controls.alpha.stringValue().to_string()
    }

    fn caret_state(&self) -> String {
        format!("{:?}|{}", self.alpha_text(), self.selection())
    }

    async fn arm(&self, cx: &mut AsyncApp, text: &str, caret: usize) {
        self.controls.window.makeFirstResponder(None);
        self.handle
            .update(cx, |keys, _, cx| {
                keys.alpha = text.to_owned();
                cx.notify();
            })
            .ok();
        for _ in 0..60 {
            sleep(cx, POLL).await;
            if self.alpha_text() == text {
                break;
            }
        }
        focus_at(cx, &self.controls.window, &self.controls.alpha, caret).await;
    }

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

    /// One script, or this could type into whatever the user focused between check and send.
    async fn run_guarded(&mut self, cx: &mut AsyncApp, body: &str, tool: &str) -> String {
        fs::write(self.dir.join("req.cliclick"), tool).ok();
        let pid = self.pid;
        let script = format!(
            "tell application \"System Events\"\n\
             \tset fg to unix id of first application process whose frontmost is true\n\
             \tif fg is {pid} then\n\
             \t\t{body}\n\
             \tend if\n\
             end tell\n\
             return (fg as string)\n"
        );
        let frontmost = self.run_script(cx, script).await;
        fs::write(self.dir.join("req.cliclick"), "").ok();
        sleep(cx, SETTLE).await;
        frontmost
    }

    /// `body` is System Events, `tool` cliclick: one cannot click, the other cannot chord a letter.
    async fn deliver(
        &mut self,
        cx: &mut AsyncApp,
        key: &'static str,
        body: &str,
        tool: &str,
    ) -> Sent {
        if !self.window_is_key() {
            cx.update(|cx| cx.activate(true));
            sleep(cx, Duration::from_millis(300)).await;
        }
        let key_window_before = self.window_is_key();
        let suppress = self.sabotage == Sabotage::NoKeys;
        let body = if suppress { "delay 0.01" } else { body };
        // Release every modifier: one lost keyup would hold it down for the rest of the run.
        let tool = if suppress || tool.is_empty() {
            String::new()
        } else {
            format!("{tool} ku:cmd,alt,ctrl,shift,fn")
        };
        let frontmost = self.run_guarded(cx, body, &tool).await;
        Sent {
            key,
            frontmost_ok: frontmost == self.pid.to_string(),
            frontmost,
            key_window_before,
            key_window_after: self.window_is_key(),
            residual_modifiers: residual_modifiers(),
        }
    }

    async fn send(&mut self, cx: &mut AsyncApp, key: &'static str, body: &str) -> Sent {
        self.deliver(cx, key, body, "").await
    }

    async fn press(&mut self, cx: &mut AsyncApp, key: &'static str, tool: &str) -> Sent {
        self.deliver(cx, key, "delay 0.02", tool).await
    }

    /// Only inside this window's frame: anywhere else the click lands in whatever app owns the pixel.
    async fn click(&mut self, cx: &mut AsyncApp, label: &'static str, (x, y): (f64, f64)) -> Sent {
        let frame = self.controls.window.frame();
        let top = self.screen_height - (frame.origin.y + frame.size.height);
        let inside = x >= frame.origin.x
            && x <= frame.origin.x + frame.size.width
            && y >= top
            && y <= top + frame.size.height;
        if !inside {
            return Sent {
                key: label,
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

    async fn probe_hosted(&mut self, cx: &mut AsyncApp, tool: &str) -> String {
        self.arm(cx, "abcdef", 3).await;
        let sent = self.press(cx, "probe", tool).await;
        format!("{} trusted={}", self.caret_state(), sent.trusted())
    }

    async fn probe_bare_field(&mut self, cx: &mut AsyncApp, tool: &str, parent: &str) -> String {
        let mtm = MainThreadMarker::new().expect("probes run on the main thread");
        let field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(10., 340.), NSSize::new(300., 24.)),
        );
        field.setStringValue(&NSString::from_str("abcdef"));

        // Below the suite's window, off the click rows; an NSWindow made in code frees itself on close.
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

        let armed = focus_at(cx, &host, &field, 3).await;
        let key = host.isKeyWindow();
        let frontmost = self.run_guarded(cx, "delay 0.02", tool).await;

        let editor_state = selection_of(&field);
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

    fn push(&mut self, row: [&str; 7]) {
        let line = row.join("\t");
        println!("{line}");
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("results.tsv"))
        {
            let _ = writeln!(file, "{line}");
        }
        let [_, verdict, ..] = row;
        self.tally[match verdict {
            "PASS" => 0,
            "FAIL" => 1,
            "VACUOUS" => 2,
            "NOKEY" => 3,
            "STUCKMOD" => 4,
            _ => 5,
        }] += 1;
    }

    fn expect(&mut self, name: &str, pre: &str, post: &str, want: &str, sent: &Sent) {
        let want = match self.sabotage {
            Sabotage::Want => format!("{want}·sabotaged"),
            _ => want.to_owned(),
        };
        let verdict = match sent.disqualified() {
            Some(reason) => reason,
            None if post != want => "FAIL",
            None if pre == want => "VACUOUS",
            None => "PASS",
        };
        self.push([
            name,
            verdict,
            pre,
            post,
            &want,
            sent.key,
            &sent.provenance(),
        ]);
    }

    fn expect_same(&mut self, name: &str, pre: &str, post: &str, same: bool, sent: &Sent) -> bool {
        let same = same != (self.sabotage == Sabotage::Want);
        let verdict = match sent.disqualified() {
            Some(reason) => reason,
            None if (pre == post) != same => "FAIL",
            None => "PASS",
        };
        let want = if same { "= pre" } else { "≠ pre" };
        self.push([name, verdict, pre, post, want, sent.key, &sent.provenance()]);
        verdict == "PASS"
    }

    fn expect_change(&mut self, name: &str, pre: &str, post: &str, sent: &Sent) {
        self.expect_same(name, pre, post, false, sent);
    }

    fn expect_stable(&mut self, name: &str, pre: &str, post: &str, sent: &Sent) {
        if self.expect_same(name, pre, post, true, sent) {
            self.controls_passed += 1;
        }
    }

    fn note(&mut self, name: &str, value: impl Display) {
        self.push([name, "NOTE", "", &value.to_string(), "", "-", ""]);
    }

    fn point_of(&self, view: &NSView) -> (f64, f64) {
        let in_window = view.convertRect_toView(view.bounds(), None);
        let on_screen = self.controls.window.convertRectToScreen(in_window);
        (
            on_screen.origin.x + on_screen.size.width / 2.,
            self.screen_height - (on_screen.origin.y + on_screen.size.height / 2.),
        )
    }

    /// Mid height in the right-hand column: no hosted view there, and clear of the titlebar band.
    fn gpui_point(&self) -> (f64, f64) {
        let frame = self.controls.window.frame();
        (
            frame.origin.x + frame.size.width - 80.,
            self.screen_height - (frame.origin.y + frame.size.height / 2.),
        )
    }

    fn model<R: Default>(&self, cx: &mut AsyncApp, read: impl FnOnce(&Keys, &Window) -> R) -> R {
        self.handle
            .update(cx, |keys, window, _| read(keys, window))
            .unwrap_or_default()
    }
}

fn menu_titles(app: &NSApplication) -> String {
    let Some(menu) = app.mainMenu() else {
        return "<no main menu>".into();
    };
    let titles: Vec<String> = menu
        .itemArray()
        .iter()
        .map(|item| item.title().to_string())
        .collect();
    if titles.is_empty() {
        "<empty main menu>".into()
    } else {
        titles.join("/")
    }
}

async fn drive(cx: &mut AsyncApp, handle: WindowHandle<Keys>, dir: PathBuf) {
    let mtm = MainThreadMarker::new().expect("gpui runs its app closure on the main thread");
    // A click tool's top-left space flips on the display at the global origin, not `screens[0]`.
    let screens = NSScreen::screens(mtm);
    let frames = || screens.iter().map(|screen| screen.frame());
    let screen_height = frames()
        .find(|frame| frame.origin.x == 0. && frame.origin.y == 0.)
        .or_else(|| frames().next())
        .map_or(0., |frame| frame.size.height);

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
        cx.update(|cx| cx.quit());
        return;
    };

    let (sabotage, sabotage_name) = match std::env::var("KEYS_SABOTAGE").as_deref() {
        Ok("want") => (Sabotage::Want, "want"),
        Ok("nokeys") => (Sabotage::NoKeys, "nokeys"),
        _ => (Sabotage::None, "none"),
    };
    let mut suite = Suite {
        dir,
        seq: 0,
        sabotage,
        handle,
        controls,
        app: NSApplication::sharedApplication(mtm),
        screen_height,
        tally: [0; 6],
        controls_passed: 0,
        pid: std::process::id(),
    };
    fs::write(suite.dir.join("who"), suite.pid.to_string()).ok();
    fs::write(suite.dir.join("req.cliclick"), "").ok();

    for _ in 0..400 {
        if suite.window_is_key() {
            break;
        }
        sleep(cx, POLL).await;
    }

    let saved_board = board();
    suite.note("env.menus", menu_titles(&suite.app));
    suite.note(
        "env.full_keyboard_access",
        suite.app.isFullKeyboardAccessEnabled(),
    );
    suite.note(
        "env.install_menus",
        match std::env::var("KEYS_MENUS").as_deref() {
            Ok("0") => "skipped",
            _ => "called",
        },
    );
    suite.note("env.sabotage", sabotage_name);
    suite.note("env.window_is_key", suite.window_is_key());
    suite.note(
        "env.process",
        format!(
            "{} pid={}",
            NSProcessInfo::processInfo().processName(),
            suite.pid
        ),
    );
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
        suite.controls.window.defaultButtonCell().is_some(),
    );
    let os_menu_bar = suite.menu_bar_from_os(cx).await;
    suite.note("env.menu_bar_from_os", os_menu_bar);

    suite.arm(cx, "abcdef", 6).await;
    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let sent = suite
        .click(cx, "click alpha", suite.point_of(&suite.controls.alpha))
        .await;
    let post = suite.responder();
    suite.expect("focus.click_field", &pre, &post, "editor:alpha", &sent);

    suite.arm(cx, "", 0).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "H e l l o", "keystroke \"Hello\"").await;
    let post = suite.alpha_text();
    suite.expect("type.plain", &pre, &post, "Hello", &sent);

    // `keystroke (character id 233)` lands a bare "a", so é goes through the ⌥e dead key.
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
    suite
        .send(cx, "char-id-233", "keystroke (character id 233)")
        .await;
    let landed = suite.alpha_text();
    suite.note("type.char_id_233_lands", format!("{landed:?}"));

    let state = suite.probe_hosted(cx, "kp:arrow-left").await;
    suite.note("probe.hosted_arrow_bare", state);
    let state = suite.probe_hosted(cx, "kd:fn kp:arrow-left ku:fn").await;
    suite.note("probe.hosted_arrow_fn", state);
    suite.arm(cx, "abcdef", 3).await;
    let sent = suite.send(cx, "left(se)", "key code 123").await;
    suite.note(
        "probe.hosted_arrow_system_events",
        format!("{} trusted={}", suite.caret_state(), sent.trusted()),
    );

    for (name, tool, parent) in [
        ("probe.bare_field_plain_window", "kp:arrow-left", "plain"),
        (
            "probe.bare_field_window_content_view",
            "kp:arrow-left",
            "content",
        ),
        ("probe.bare_field_gpui_view", "kp:arrow-left", "gpui"),
        ("probe.bare_field_plain_home", "kp:home", "plain"),
        ("probe.bare_field_plain_end", "kp:end", "plain"),
    ] {
        let state = suite.probe_bare_field(cx, tool, parent).await;
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

    // Caret rows assert the text too: an inserted character also moves the location by one.
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

    // Home and End scroll the field editor rather than move a single-line caret: assert stability.
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

    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.selection();
    let sent = suite
        .send(cx, "cmd-a", "keystroke \"a\" using {command down}")
        .await;
    let post = suite.selection();
    suite.expect("edit.cmd_a_select_all", &pre, &post, "(0,6)", &sent);

    suite.arm(cx, "abcdef", 0).await;
    suite.select(0, 6);
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
    suite.select(0, 6);
    set_board("sentinel-cut");
    let pre = format!("text={}|board={}", suite.alpha_text(), board());
    let sent = suite
        .send(cx, "cmd-x", "keystroke \"x\" using {command down}")
        .await;
    let post = format!("text={}|board={}", suite.alpha_text(), board());
    suite.expect("edit.cmd_x_cut", &pre, &post, "text=|board=abcdef", &sent);

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

    // The undo here runs through the undo manager rather than ⌘Z, so a red ⌘Z cannot mask it.
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

    // The text rides along: a responder-only row cannot tell a Tab typed as U+0009 from a focus move.
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

    // Named, not `expect_change`: a key loop closed into a ring among the hosted views must fail.
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
    let clicks_before = suite.model(cx, |keys, _| keys.gpui_clicks);
    let pre = format!("resp={}|gpui_clicks={}", suite.responder(), clicks_before);
    let sent = suite.click(cx, "click gpui", suite.gpui_point()).await;
    let (clicks_after, gpui_focused) = suite.model(cx, |keys, window| {
        (keys.gpui_clicks, keys.focus.is_focused(window))
    });
    let post = format!("resp={}|gpui_clicks={}", suite.responder(), clicks_after);
    suite.expect(
        "focus.click_gpui_returns_responder",
        &pre,
        &post,
        &format!("resp=gpui|gpui_clicks={}", clicks_before + 1),
        &sent,
    );
    suite.note("focus.gpui_handle_focused", gpui_focused);

    let no_key = Sent {
        key: "-",
        frontmost: String::new(),
        frontmost_ok: true,
        key_window_before: true,
        key_window_after: true,
        residual_modifiers: String::new(),
    };
    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let pre = suite.responder();
    let took = suite
        .controls
        .window
        .makeFirstResponder(Some(&suite.controls.toggle));
    sleep(cx, POLL).await;
    let post = suite.responder();
    suite.note("control.switch_makeFirstResponder", took);
    suite.expect("control.switch_takes_focus", &pre, &post, "switch", &no_key);

    // `want` is `!before`, never `!post`, which no outcome could equal.
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
    suite.note("control.slider_makeFirstResponder", took);
    suite.expect("control.slider_takes_focus", &pre, &post, "slider", &no_key);

    let pre = format!("{:.4}", suite.controls.slider.doubleValue());
    let sent = suite.press(cx, "right", "kp:arrow-right").await;
    let post = format!("{:.4}", suite.controls.slider.doubleValue());
    suite.expect_change("control.arrow_moves_slider", &pre, &post, &sent);

    suite.controls.window.makeFirstResponder(None);
    sleep(cx, POLL).await;
    let default_before = suite.model(cx, |keys, _| keys.default_clicks);
    let sent = suite.press(cx, "return", "kp:return").await;
    let default_after = suite.model(cx, |keys, _| keys.default_clicks);
    suite.expect(
        "control.return_fires_default_button",
        &default_before.to_string(),
        &default_after.to_string(),
        &(default_before + 1).to_string(),
        &sent,
    );

    suite.arm(cx, "abcdef", 6).await;
    let submits_before = suite.model(cx, |keys, _| keys.submits);
    let sent = suite.press(cx, "return", "kp:return").await;
    let submits_after = suite.model(cx, |keys, _| keys.submits);
    suite.expect(
        "control.return_in_field_submits",
        &submits_before.to_string(),
        &submits_after.to_string(),
        &(submits_before + 1).to_string(),
        &sent,
    );

    // Escape reaches `cancelOperation:` and keeps the edit; this catches it arriving as U+001B.
    suite.arm(cx, "abcdef", 6).await;
    let pre = suite.alpha_text();
    let sent = suite.send(cx, "Z", "keystroke \"Z\"").await;
    let typed = suite.alpha_text();
    suite.expect("control.escape_setup_typed", &pre, &typed, "abcdefZ", &sent);
    let sent = suite.press(cx, "escape", "kp:esc").await;
    let post = suite.alpha_text();
    suite.expect_stable("control.escape_leaves_field_alone", &typed, &post, &sent);

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

    set_board(&saved_board);
    let [pass, fail, vacuous, nokey, stuckmod, notes] = suite.tally;
    suite.note(
        "summary",
        format!(
            "pass={pass} fail={fail} vacuous={vacuous} nokey={nokey} stuckmod={stuckmod} \
             notes={notes} control_rows_passing={}",
            suite.controls_passed
        ),
    );

    fs::write(suite.dir.join("done"), "1").ok();
    sleep(cx, Duration::from_millis(120)).await;
    cx.update(|cx| cx.quit());
}

fn main() {
    let dir = PathBuf::from(std::env::var("KEYS_DIR").unwrap_or_else(|_| "/tmp/ng-keys".into()));
    fs::create_dir_all(&dir).ok();

    gpui_platform::application().run(move |cx: &mut App| {
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
