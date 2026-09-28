use std::cell::RefCell;
use std::path::PathBuf;

use finder_fs::ops::{ImageFormat, ImageSize, Op, Quick};
use gpui::Context;
use native_gpui::MenuEntry;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSButton, NSControlStateValueOn, NSPopUpButton,
    NSTextAlignment, NSTextField, NSView, NSWorkspace,
};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize, NSString, NSURL};

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
            QuickAct::RemoveBackground => {
                return self.run_op(Op::Quick(Quick::RemoveBackground(items)), After::Rename, cx);
            }
        };
        self.run_op(Op::Quick(quick), After::Select, cx);
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
            this.update(cx, |this, cx| this.run_op(op, After::Select, cx))
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

struct RetitleIvars {
    formats: RefCell<Option<Retained<NSPopUpButton>>>,
    convert: RefCell<Option<Retained<NSButton>>>,
}

define_class!(
    /// Names the convert button after the format chosen, as Finder's does.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = RetitleIvars]
    struct Retitle;

    impl Retitle {
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
    }
);

fn convert_title(format: ImageFormat) -> String {
    format!("Convert to {}", format.title())
}

fn convert_choice(mtm: MainThreadMarker) -> Option<(ImageFormat, ImageSize, bool)> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Convert Image"));
    let convert = alert.addButtonWithTitle(&NSString::from_str(&convert_title(ImageFormat::Jpeg)));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 380., 90.));
    view.addSubview(&label(mtm, "Format:", rect(0., 64., 72., 18.)));
    view.addSubview(&label(mtm, "Image Size:", rect(0., 32., 72., 18.)));
    let formats: Vec<&str> = ImageFormat::ALL
        .iter()
        .map(|format| format.title())
        .collect();
    let sizes: Vec<&str> = ImageSize::ALL.iter().map(|size| size.title()).collect();
    let format_popup = popup(mtm, &formats, rect(76., 60., 302., 26.));
    let size_popup = popup(mtm, &sizes, rect(76., 28., 302., 26.));
    let keep = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Preserve Metadata"),
            None,
            None,
            mtm,
        )
    };
    keep.setState(NSControlStateValueOn);
    keep.setFrame(rect(76., 2., 200., 18.));
    view.addSubview(&format_popup);
    view.addSubview(&size_popup);
    view.addSubview(&keep);
    let retitle = Retitle::alloc(mtm).set_ivars(RetitleIvars {
        formats: RefCell::new(Some(format_popup.clone())),
        convert: RefCell::new(Some(convert)),
    });
    let retitle: Retained<Retitle> = unsafe { msg_send![super(retitle), init] };
    unsafe {
        format_popup.setTarget(Some(&retitle));
        format_popup.setAction(Some(sel!(ngRetitle:)));
    }
    alert.setAccessoryView(Some(&view));
    let answer = alert.runModal();
    drop(retitle);
    if answer != NSAlertFirstButtonReturn {
        return None;
    }
    let pick = |popup: &NSPopUpButton| popup.indexOfSelectedItem().max(0) as usize;
    Some((
        ImageFormat::ALL[pick(&format_popup).min(2)],
        ImageSize::ALL[pick(&size_popup).min(3)],
        keep.state() == NSControlStateValueOn,
    ))
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
