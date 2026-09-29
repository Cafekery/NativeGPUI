use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use block2::RcBlock;
use finder_fs::ops::{Op, Quick};
use gpui::{AsyncApp, Context, WeakEntity};
use objc2::encode::{Encode, Encoding};
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertThirdButtonReturn, NSBackingStoreType,
    NSModalResponse, NSModalResponseOK, NSSavePanel, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSURL,
};

use crate::app::FinderDemo;
use crate::ops::After;

#[link(name = "AVKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMTimeGetSeconds(time: Time) -> f64;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Time {
    value: i64,
    scale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl Encode for Time {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

impl Time {
    pub(crate) fn seconds(self) -> Option<f64> {
        const VALID: u32 = 1;
        let seconds = unsafe { CMTimeGetSeconds(self) };
        (self.flags & VALID != 0 && seconds.is_finite()).then_some(seconds)
    }
}

const WIDTH: f64 = 370.;
const POLL: Duration = Duration::from_millis(20);
const POLLS: u32 = 250;
const TRIM_OK: NSInteger = 0;

struct Ivars {
    item: PathBuf,
    window: Retained<NSWindow>,
    view: Retained<NSView>,
    finder: WeakEntity<FinderDemo>,
    app: AsyncApp,
}

thread_local! {
    static OPEN: RefCell<Vec<Retained<Trimmer>>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Trimmer;

    unsafe impl NSObjectProtocol for Trimmer {}

    unsafe impl NSWindowDelegate for Trimmer {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            if let Some(player) = self.player() {
                unsafe { msg_send![&player, pause] }
            }
            self.ivars().window.setDelegate(None);
            let closed = OPEN.with_borrow_mut(|open| {
                let at = open.iter().position(|open| std::ptr::eq(&**open, self))?;
                Some(open.swap_remove(at))
            });
            self.ivars().app.spawn(async move |_| drop(closed)).detach();
        }
    }
);

pub(crate) fn trim(item: PathBuf, cx: &mut Context<FinderDemo>) {
    cx.spawn(async move |finder, cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(trimmer) = Trimmer::new(mtm, item, finder, cx.clone()) else {
            return;
        };
        for _ in 0..POLLS {
            if trimmer.can_trim() {
                trimmer.show();
                trimmer.begin();
                return;
            }
            cx.background_executor().timer(POLL).await;
        }
        trimmer.ivars().window.close();
    })
    .detach();
}

impl Trimmer {
    fn new(
        mtm: MainThreadMarker,
        item: PathBuf,
        finder: WeakEntity<FinderDemo>,
        app: AsyncApp,
    ) -> Option<Retained<Self>> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(item.to_str()?));
        let player: Option<Retained<AnyObject>> =
            unsafe { msg_send![AnyClass::get(c"AVPlayer")?, playerWithURL: &*url] };
        let view: Retained<NSView> = unsafe {
            let made: Allocated<NSView> = msg_send![AnyClass::get(c"AVPlayerView")?, alloc];
            msg_send![made, initWithFrame: rect(WIDTH, WIDTH)]
        };
        unsafe { msg_send![&view, setPlayer: &*player?] }
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(WIDTH, WIDTH),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&item.file_name()?.to_string_lossy()));
        window.setContentView(Some(&view));
        let trimmer = Trimmer::alloc(mtm).set_ivars(Ivars {
            item,
            window,
            view,
            finder,
            app,
        });
        let trimmer: Retained<Trimmer> = unsafe { msg_send![super(trimmer), init] };
        trimmer
            .ivars()
            .window
            .setDelegate(Some(ProtocolObject::from_ref(&*trimmer)));
        OPEN.with_borrow_mut(|open| open.push(trimmer.clone()));
        Some(trimmer)
    }

    fn player(&self) -> Option<Retained<AnyObject>> {
        unsafe { msg_send![&self.ivars().view, player] }
    }

    fn player_item(&self) -> Option<Retained<AnyObject>> {
        unsafe { msg_send![&self.player()?, currentItem] }
    }

    fn name(&self) -> String {
        self.ivars()
            .item
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn can_trim(&self) -> bool {
        unsafe { msg_send![&self.ivars().view, canBeginTrimming] }
    }

    fn show(&self) {
        let window = &self.ivars().window;
        let size: NSSize = self.player_item().map_or(NSSize::ZERO, |item| unsafe {
            msg_send![&item, presentationSize]
        });
        if size.width > 0. && size.height > 0. {
            window.setContentSize(NSSize::new(
                WIDTH,
                (WIDTH * size.height / size.width).round(),
            ));
        }
        window.center();
        window.makeKeyAndOrderFront(None);
    }

    fn begin(&self) {
        let trimmer = Weak::new(self);
        let trimmed = RcBlock::new(move |result: NSInteger| {
            if let Some(trimmer) = trimmer.load() {
                trimmer.trimmed(result);
            }
        });
        unsafe { msg_send![&self.ivars().view, beginTrimmingWithCompletionHandler: &*trimmed] }
    }

    fn trimmed(&self, result: NSInteger) {
        match self.range().filter(|_| result == TRIM_OK) {
            Some((start, end)) => self.ask(start, end),
            None => self.close(),
        }
    }

    fn range(&self) -> Option<(f64, f64)> {
        let item = self.player_item()?;
        let length = Time::seconds(unsafe { msg_send![&item, duration] })?;
        let start = Time::seconds(unsafe { msg_send![&item, reversePlaybackEndTime] });
        let end = Time::seconds(unsafe { msg_send![&item, forwardPlaybackEndTime] });
        let (start, end) = (start.unwrap_or(0.), end.unwrap_or(length));
        (start > 0. || end < length).then_some((start, end))
    }

    fn ask(&self, start: f64, end: f64) {
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(&format!(
            "Do you want to save the changes made to the document \u{201c}{}\u{201d}?",
            self.name()
        )));
        alert.setInformativeText(&NSString::from_str(
            "You can choose to save your changes, or save as a new clip. You can\u{2019}t undo this action.",
        ));
        for title in ["New Clip", "Cancel", "Replace"] {
            alert.addButtonWithTitle(&NSString::from_str(title));
        }
        let trimmer = Weak::new(self);
        let answered = RcBlock::new(move |response: NSModalResponse| {
            let Some(trimmer) = trimmer.load() else {
                return;
            };
            if response == NSAlertFirstButtonReturn {
                trimmer.ask_where(start, end);
            } else if response == NSAlertThirdButtonReturn {
                trimmer.save(trimmer.ivars().item.clone(), Vec::new(), start, end);
            } else {
                trimmer.later(Trimmer::begin);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&self.ivars().window, Some(&answered));
    }

    fn ask_where(&self, start: f64, end: f64) {
        let panel = NSSavePanel::savePanel(self.mtm());
        panel.setNameFieldStringValue(&NSString::from_str(&self.name()));
        panel.setShowsTagField(true);
        if let Some(kind) = self.content_type() {
            let kinds = NSArray::from_retained_slice(&[kind]);
            unsafe { msg_send![&panel, setAllowedContentTypes: &*kinds] }
        }
        let trimmer = Weak::new(self);
        let asked = panel.clone();
        let chosen = RcBlock::new(move |response: NSModalResponse| {
            let Some(trimmer) = trimmer.load() else {
                return;
            };
            let into = asked.URL().and_then(|url| url.path());
            match into.filter(|_| response == NSModalResponseOK) {
                Some(into) => {
                    let tags = asked
                        .tagNames()
                        .map(|names| names.iter().map(|name| name.to_string()).collect())
                        .unwrap_or_default();
                    trimmer.save(PathBuf::from(into.to_string()), tags, start, end);
                }
                None => trimmer.later(Trimmer::begin),
            }
        });
        panel.beginSheetModalForWindow_completionHandler(&self.ivars().window, &chosen);
    }

    fn content_type(&self) -> Option<Retained<AnyObject>> {
        let ext = NSString::from_str(self.ivars().item.extension()?.to_str()?);
        unsafe { msg_send![AnyClass::get(c"UTType")?, typeWithFilenameExtension: &*ext] }
    }

    fn save(&self, into: PathBuf, tags: Vec<String>, start: f64, end: f64) {
        let ivars = self.ivars();
        let op = Op::Quick(Quick::Trim {
            item: ivars.item.clone(),
            start,
            end,
            into,
            tags,
        });
        let mut app = ivars.app.clone();
        ivars
            .finder
            .update(&mut app, |finder, cx| finder.run_op(op, After::Nothing, cx))
            .ok();
        self.close();
    }

    fn close(&self) {
        self.later(|trimmer| trimmer.ivars().window.close());
    }

    /// AppKit takes a sheet down only after its handler returns.
    fn later(&self, then: impl FnOnce(&Trimmer) + 'static) {
        let trimmer = Weak::new(self);
        self.ivars()
            .app
            .spawn(async move |_| {
                if let Some(trimmer) = trimmer.load() {
                    then(&trimmer);
                }
            })
            .detach();
    }
}

fn rect(width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::ZERO, NSSize::new(width, height))
}
