use std::cell::RefCell;
use std::path::PathBuf;

use finder_fs::ops::{ImageFormat, ImageSize, Op, Quick};
use gpui::Context;
use native_gpui::MenuEntry;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSControlStateValueOn, NSModalResponseCancel,
    NSModalResponseOK, NSPopUpButton, NSTextAlignment, NSTextField, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::app::FinderDemo;
use crate::data::Face;
use crate::menus::Act;
use crate::ops::After;

/// Finder's Quick Actions that public APIs reach. Markup and Trim run inside Quick Look's own
/// editing modes, which no public API opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuickAct {
    RotateLeft,
    RotateRight,
    CreatePdf,
    ConvertImage,
    RemoveBackground,
    Customize,
}

impl QuickAct {
    pub(crate) fn title(self) -> &'static str {
        match self {
            QuickAct::RotateLeft => "Rotate Left",
            QuickAct::RotateRight => "Rotate Right",
            QuickAct::CreatePdf => "Create PDF",
            QuickAct::ConvertImage => "Convert Image",
            QuickAct::RemoveBackground => "Remove Background",
            QuickAct::Customize => "Customize\u{2026}",
        }
    }

    pub(crate) fn symbol(self) -> &'static str {
        match self {
            QuickAct::RotateLeft => "rotate.left",
            QuickAct::RotateRight => "rotate.right",
            QuickAct::CreatePdf => "doc.richtext",
            QuickAct::ConvertImage => "photo.on.rectangle.angled",
            QuickAct::RemoveBackground => "person.and.background.dotted",
            QuickAct::Customize => "ellipsis.circle",
        }
    }
}

/// What Finder offers for a selection of these kinds, in its order; Rotate Right is Rotate
/// Left's Option alternate.
pub(crate) fn offered(faces: &[Face]) -> Vec<QuickAct> {
    if faces.is_empty() {
        return Vec::new();
    }
    let all = |pass: fn(Face) -> bool| faces.iter().all(|face| pass(*face));
    let mut acts = Vec::new();
    if all(|face| matches!(face, Face::Image | Face::Movie)) {
        acts.push(QuickAct::RotateLeft);
    }
    if all(|face| matches!(face, Face::Image | Face::Pdf))
        && (faces.len() > 1 || faces[0] == Face::Image)
    {
        acts.push(QuickAct::CreatePdf);
    }
    if all(|face| face == Face::Image) {
        acts.push(QuickAct::ConvertImage);
        acts.push(QuickAct::RemoveBackground);
    }
    acts
}

/// The rows of a Quick Actions menu: `acts`, then Customize… under a rule.
pub(crate) fn menu_rows(acts: &[QuickAct]) -> Vec<(MenuEntry, Act)> {
    let mut rows = Vec::new();
    for act in acts {
        rows.push((MenuEntry::item(act.title()), Act::Quick(*act)));
        if *act == QuickAct::RotateLeft {
            rows.push((
                MenuEntry::item(QuickAct::RotateRight.title()).option_alternate(),
                Act::Quick(QuickAct::RotateRight),
            ));
        }
    }
    if !rows.is_empty() {
        rows.push((MenuEntry::separator(), Act::Nothing));
    }
    rows.push((
        MenuEntry::item(QuickAct::Customize.title()),
        Act::Quick(QuickAct::Customize),
    ));
    rows
}

const EXTENSIONS: &str = "x-apple.systempreferences:com.apple.ExtensionsPreferences?extensionPointIdentifier=com.apple.finder-quick-actions";

impl FinderDemo {
    pub(crate) fn chosen_faces(&self) -> Vec<Face> {
        self.chosen_lines()
            .iter()
            .map(|line| line.row.face)
            .collect()
    }

    pub(crate) fn run_quick(&mut self, act: QuickAct, cx: &mut Context<Self>) {
        let items = self.paths();
        let quick = match act {
            QuickAct::Customize => {
                if let Some(url) = NSURL::URLWithString(&NSString::from_str(EXTENSIONS)) {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
                return;
            }
            _ if items.is_empty() => return,
            QuickAct::ConvertImage => return self.ask_convert(items, cx),
            QuickAct::RotateLeft | QuickAct::RotateRight => Quick::Rotate {
                items,
                left: act == QuickAct::RotateLeft,
            },
            QuickAct::CreatePdf => Quick::CreatePdf(items),
            QuickAct::RemoveBackground => Quick::RemoveBackground(items),
        };
        // Finder puts a file it makes straight into renaming.
        let after = match quick {
            Quick::Rotate { .. } => After::Select,
            _ => After::Rename,
        };
        self.run_op(Op::Quick(quick), after, cx);
    }

    /// Finder's Convert Image window: format, size and metadata, asked once for the selection.
    fn ask_convert(&self, items: Vec<PathBuf>, cx: &mut Context<Self>) {
        // From a task, outside GPUI's borrow of the app, which the modal loop would re-enter.
        cx.spawn(async move |this, cx| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some((format, size, metadata)) = convert_choice(mtm) else {
                return;
            };
            let op = Op::Quick(Quick::Convert {
                items,
                format,
                size,
                metadata,
            });
            this.update(cx, |this, cx| this.run_op(op, After::Rename, cx))
                .ok();
        })
        .detach();
    }
}

fn label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setAlignment(NSTextAlignment::Right);
    field.setFrame(frame);
    field
}

fn popup(mtm: MainThreadMarker, titles: &[&str], frame: NSRect) -> Retained<NSPopUpButton> {
    let button = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), frame, false);
    for title in titles {
        button.addItemWithTitle(&NSString::from_str(title));
    }
    button
}

struct DialogIvars {
    formats: RefCell<Option<Retained<NSPopUpButton>>>,
    convert: RefCell<Option<Retained<NSButton>>>,
}

define_class!(
    /// Finder's Convert Image window's controller: names the convert button after the format and
    /// ends the modal session.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DialogIvars]
    struct Dialog;

    unsafe impl NSObjectProtocol for Dialog {}

    unsafe impl NSWindowDelegate for Dialog {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: *mut AnyObject) -> bool {
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseCancel);
            true
        }
    }

    impl Dialog {
        #[unsafe(method(ngRetitle:))]
        fn ng_retitle(&self, _sender: *mut AnyObject) {
            let (Some(formats), Some(convert)) =
                (&*self.ivars().formats.borrow(), &*self.ivars().convert.borrow())
            else {
                return;
            };
            let at = formats.indexOfSelectedItem().max(0) as usize;
            let format = ImageFormat::ALL[at.min(ImageFormat::ALL.len() - 1)];
            convert.setTitle(&NSString::from_str(&convert_title(format)));
        }

        #[unsafe(method(ngConvert:))]
        fn ng_convert(&self, _sender: *mut AnyObject) {
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseOK);
        }

        #[unsafe(method(ngCancel:))]
        fn ng_cancel(&self, _sender: *mut AnyObject) {
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseCancel);
        }
    }
);

fn convert_title(format: ImageFormat) -> String {
    format!("Convert to {}", format.title())
}

fn push_button(
    mtm: MainThreadMarker,
    title: &str,
    key: &str,
    target: &Dialog,
    action: objc2::runtime::Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setKeyEquivalent(&NSString::from_str(key));
    button.setFrame(frame);
    button
}

/// Frames are Finder's, measured over Accessibility, in a 426 × 191 content view.
fn convert_choice(mtm: MainThreadMarker) -> Option<(ImageFormat, ImageSize, bool)> {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0., 0., 426., 191.),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Convert Image"));
    let view = window.contentView()?;
    let formats: Vec<&str> = ImageFormat::ALL
        .iter()
        .map(|format| format.title())
        .collect();
    let sizes: Vec<&str> = ImageSize::ALL.iter().map(|size| size.title()).collect();
    let format_popup = popup(mtm, &formats, rect(100., 141., 302., 26.));
    let size_popup = popup(mtm, &sizes, rect(100., 109., 302., 26.));
    let keep = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Preserve Metadata"),
            None,
            None,
            mtm,
        )
    };
    keep.setState(NSControlStateValueOn);
    keep.setFrame(rect(100., 85., 138., 18.));
    let dialog = Dialog::alloc(mtm).set_ivars(DialogIvars {
        formats: RefCell::new(Some(format_popup.clone())),
        convert: RefCell::new(None),
    });
    let dialog: Retained<Dialog> = unsafe { msg_send![super(dialog), init] };
    let cancel = push_button(
        mtm,
        "Cancel",
        "\u{1b}",
        &dialog,
        sel!(ngCancel:),
        rect(202., 24., 68., 26.),
    );
    let convert = push_button(
        mtm,
        &convert_title(ImageFormat::Jpeg),
        "\r",
        &dialog,
        sel!(ngConvert:),
        rect(276., 24., 126., 26.),
    );
    *dialog.ivars().convert.borrow_mut() = Some(convert.clone());
    unsafe {
        format_popup.setTarget(Some(&dialog));
        format_popup.setAction(Some(sel!(ngRetitle:)));
    }
    view.addSubview(&label(mtm, "Format:", rect(47., 149., 49., 18.)));
    view.addSubview(&label(mtm, "Image Size:", rect(24., 117., 72., 18.)));
    view.addSubview(&format_popup);
    view.addSubview(&size_popup);
    view.addSubview(&keep);
    view.addSubview(&cancel);
    view.addSubview(&convert);
    window.setDelegate(Some(ProtocolObject::from_ref(&*dialog)));
    window.center();
    let answer = NSApplication::sharedApplication(mtm).runModalForWindow(&window);
    window.orderOut(None);
    window.setDelegate(None);
    if answer != NSModalResponseOK {
        return None;
    }
    let pick = |popup: &NSPopUpButton| popup.indexOfSelectedItem().max(0) as usize;
    Some((
        ImageFormat::ALL[pick(&format_popup).min(ImageFormat::ALL.len() - 1)],
        ImageSize::ALL[pick(&size_popup).min(ImageSize::ALL.len() - 1)],
        keep.state() == NSControlStateValueOn,
    ))
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
