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
    NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use crate::app::FinderDemo;
use crate::data::Face;
use crate::menus::Act;
use crate::ops::After;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuickAct {
    RotateLeft,
    RotateRight,
    #[cfg(feature = "private-api")]
    Markup,
    Trim,
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
            #[cfg(feature = "private-api")]
            QuickAct::Markup => "Markup",
            QuickAct::Trim => "Trim",
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
            #[cfg(feature = "private-api")]
            QuickAct::Markup => "pencil.tip.crop.circle",
            QuickAct::Trim => "scissors",
            QuickAct::CreatePdf => "doc.richtext",
            QuickAct::ConvertImage => "photo.on.rectangle.angled",
            QuickAct::RemoveBackground => "person.and.background.dotted",
            QuickAct::Customize => "ellipsis.circle",
        }
    }

    pub(crate) fn alternate(self) -> Option<QuickAct> {
        (self == QuickAct::RotateLeft).then_some(QuickAct::RotateRight)
    }
}

pub(crate) fn offered(faces: &[Face]) -> Vec<QuickAct> {
    if faces.is_empty() {
        return Vec::new();
    }
    let all = |pass: fn(Face) -> bool| faces.iter().all(|face| pass(*face));
    let mut acts = Vec::new();
    if all(|face| matches!(face, Face::Image | Face::Movie)) {
        acts.push(QuickAct::RotateLeft);
    }
    #[cfg(feature = "private-api")]
    if let [Face::Image | Face::Pdf] = faces {
        acts.push(QuickAct::Markup);
    }
    if faces == [Face::Movie] {
        acts.push(QuickAct::Trim);
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

pub(crate) fn menu_rows(acts: &[QuickAct]) -> Vec<(MenuEntry, Act)> {
    let mut rows = Vec::new();
    for act in acts {
        rows.push((MenuEntry::item(act.title()), Act::Quick(*act)));
        if let Some(alternate) = act.alternate() {
            rows.push((
                MenuEntry::item(alternate.title()).option_alternate(),
                Act::Quick(alternate),
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
        let mut items = self.paths();
        let quick = match act {
            QuickAct::Customize => return cx.open_url(EXTENSIONS),
            _ if items.is_empty() => return,
            QuickAct::ConvertImage => return self.ask_convert(items, cx),
            QuickAct::Trim => return crate::trim::trim(items.swap_remove(0), cx),
            #[cfg(feature = "private-api")]
            QuickAct::Markup => return self.markup(items.swap_remove(0), cx),
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

    /// Quick Look answers to the window whose keys it takes: the main window, even from the pane.
    #[cfg(feature = "private-api")]
    fn markup(&mut self, item: PathBuf, cx: &mut Context<Self>) {
        self.ql_shown = self.cursor.clone().map(|key| (key, self.ql_icon.bounds()));
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                window
                    .update(cx, |_, window, _| {
                        native_gpui::quick_look_markup(window, item)
                    })
                    .ok();
            }
        });
    }

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
    formats: Retained<NSPopUpButton>,
    convert: RefCell<Option<Retained<NSButton>>>,
}

define_class!(
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
            if let Some(convert) = &*self.ivars().convert.borrow() {
                let format = picked(&self.ivars().formats, &ImageFormat::ALL);
                convert.setTitle(&NSString::from_str(&convert_title(format)));
            }
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

fn picked<T: Copy>(popup: &NSPopUpButton, all: &[T]) -> T {
    all[(popup.indexOfSelectedItem().max(0) as usize).min(all.len() - 1)]
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
    crate::chrome::toolbar::system_corners_of(&window);
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
        formats: format_popup.clone(),
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
    Some((
        picked(&format_popup, &ImageFormat::ALL),
        picked(&size_popup, &ImageSize::ALL),
        keep.state() == NSControlStateValueOn,
    ))
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
