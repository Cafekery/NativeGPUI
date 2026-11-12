//! Opt-in wall-clock launch telemetry. Presented handlers may run off-main.
use std::{
    fs::OpenOptions,
    io::Write,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

fn sink() -> Option<&'static std::ffi::OsStr> {
    static SINK: OnceLock<Option<std::ffi::OsString>> = OnceLock::new();
    SINK.get_or_init(|| std::env::var_os("GPUI_LAUNCH_TRACE"))
        .as_deref()
}

pub(crate) fn emit(fields: &str) {
    let Some(sink) = sink() else { return };
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |time| time.as_secs_f64() * 1000.0);
    let line = format!("GPUI_TRACE {fields} epoch_ms={epoch_ms:.3}");
    if sink == "1" {
        eprintln!("{line}");
    } else if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(sink) {
        let _ = writeln!(file, "{line}");
    }
}

pub(crate) fn phase(name: &str) {
    emit(&format!("phase={name}"));
}

// Scheduling readiness is deliberately distinct from verified scanout telemetry.
// Some macOS display paths always report presentedTime=0 (e.g. screen sharing).
// Metal presented-handler early release is deferred: Metal lives in git dep
// gpui_apple and this crate does not fork it. Commits are counted from
// MacWindow::draw after renderer.draw returns (command buffer already
// committed inside gpui_apple). Safety is still 250 ms / three draws.
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
thread_local! {
    static AFTER_DRAWABLE: std::cell::RefCell<Vec<Box<dyn FnOnce()>>> = Default::default();
}

/// Run on the next main-queue turn after first-drawable scheduling is ready.
/// This is a scheduling boundary, NOT proof of on-screen pixels: presentedTime
/// may be zero on remote/compositor display paths. Safety release is 250 ms
/// after first commit, or three submitted drawables. On this roll the Metal
/// presented handler is not hooked (gpui_apple), so those safeties are the
/// release. Register on main and capture weak entities if the window may close
/// first. Late registration is also asynchronous, never reentrant.
pub fn on_first_drawable_completed(callback: impl FnOnce() + 'static) {
    use objc::{class, msg_send, sel, sel_impl};
    let is_main: cocoa::base::BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
    assert!(
        is_main == cocoa::base::YES,
        "launch callbacks require main thread"
    );
    AFTER_DRAWABLE.with(|queue| queue.borrow_mut().push(Box::new(callback)));
    if READY.load(std::sync::atomic::Ordering::Acquire) {
        schedule_after_drawable();
    }
}

/// Compatibility spelling for launch clients. Like `on_first_drawable_completed`,
/// this schedules work after submission/compositor handoff, not verified scanout.
pub fn on_first_present(callback: impl FnOnce() + 'static) {
    on_first_drawable_completed(callback);
}

pub(crate) fn drawable_committed() {
    static COMMITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if READY.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let count = COMMITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count == 1 {
        emit("milestone=first_drawable_committed");
        let when = dispatch2::DispatchTime::NOW.time(250_000_000);
        let _ = dispatch2::DispatchQueue::main().after(when, || release("250ms_safety"));
    } else if count >= 3 {
        release("three_drawable_safety");
    }
}

fn release(reason: &str) {
    if !READY.swap(true, std::sync::atomic::Ordering::AcqRel) {
        emit(&format!("phase=after_drawable_ready reason={reason}"));
        schedule_after_drawable();
    }
}

fn schedule_after_drawable() {
    dispatch2::DispatchQueue::main().exec_async(|| {
        let callbacks = AFTER_DRAWABLE.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
        for callback in callbacks {
            callback();
        }
    });
}

pub(crate) struct Span(&'static str);
impl Span {
    pub(crate) fn new(name: &'static str) -> Self {
        phase(&format!("{name}_begin"));
        Self(name)
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        phase(&format!("{}_end", self.0));
    }
}
