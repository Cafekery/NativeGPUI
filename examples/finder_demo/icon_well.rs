use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

use block2::StackBlock;
use finder_fs::custom_icon;
use gpui::{App, SharedString};
use native_gpui::tokens::has_custom_icon;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSApplication, NSBeep, NSBezierPath, NSBitmapFormat,
    NSBitmapImageRep, NSCompositingOperation, NSControl, NSDeviceRGBColorSpace, NSDragOperation,
    NSDraggingInfo, NSGraphicsContext, NSImage, NSImageScaling, NSImageView, NSMenuItem,
    NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeURL, NSResponder,
    NSUserInterfaceItemIdentification, NSView, NSWorkspace, NSWorkspaceIconCreationOptions,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSData, NSDictionary, NSMutableData, NSNumber, NSObject, NSPoint,
    NSRect, NSSize, NSString, NSURL, NSURLContentTypeKey, ns_string,
};

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithData(
        data: &NSMutableData,
        kind: &NSString,
        count: usize,
        options: *const NSDictionary,
    ) -> *mut AnyObject;
    fn CGImageDestinationAddImage(
        destination: &AnyObject,
        image: *const c_void,
        properties: &NSDictionary<NSString, NSNumber>,
    );
    fn CGImageDestinationFinalize(destination: &AnyObject) -> bool;
    static kCGImagePropertyDPIWidth: &'static NSString;
    static kCGImagePropertyDPIHeight: &'static NSString;
}

const ICNS: &str = "com.apple.icns";
const IMAGE: &str = "public.image";
// Finder makes an icon of an image only up to 128 points, centred square on its longer side.
const IMAGE_SIZES: [f64; 3] = [16., 32., 128.];
const ICNS_SIZES: [f64; 5] = [16., 32., 128., 256., 512.];
const SCALES: [f64; 2] = [1., 2.];
const SAME_AT_MOST: usize = 100;
const SAME_SIDE: f64 = 32.;
const PAGE_RADIUS: f64 = 4.;

type Icons = Vec<(PathBuf, Option<Vec<u8>>)>;

struct Change {
    label: &'static str,
    before: Icons,
    after: Icons,
}

thread_local! {
    static UNDO: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    static REDO: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static WELLS: RefCell<Vec<Weak<IconWell>>> = const { RefCell::new(Vec::new()) };
    static PROMISED: RefCell<Option<Retained<Promise>>> = const { RefCell::new(None) };
}

pub(crate) fn generation() -> u64 {
    GENERATION.get()
}

pub(crate) enum Event {
    Changed(Vec<PathBuf>),
    Copying { path: PathBuf, change: isize },
}

pub(crate) struct WellIvars {
    paths: Vec<PathBuf>,
    same: Cell<bool>,
    shown: Cell<Option<(u64, bool)>>,
    handed: Cell<bool>,
    event: RefCell<Option<Event>>,
}

define_class!(
    #[unsafe(super(NSImageView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoIconWell"]
    #[ivars = WellIvars]
    pub(crate) struct IconWell;

    impl IconWell {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            let action = item.action();
            if action == Some(sel!(copy:)) {
                self.can_copy()
            } else if action == Some(sel!(cut:)) {
                self.can_copy() && self.can_clear()
            } else if action == Some(sel!(delete:)) {
                self.can_clear()
            } else if action == Some(sel!(paste:)) {
                self.can_paste()
            } else if action == Some(sel!(undo:)) {
                let label = labels().0;
                item.setTitle(&titled("Undo", label));
                self.can_paste() && label.is_some()
            } else if action == Some(sel!(redo:)) {
                let label = labels().1;
                item.setTitle(&titled("Redo", label));
                self.can_paste() && label.is_some()
            } else {
                unsafe { msg_send![super(self), validateMenuItem: item] }
            }
        }

        #[unsafe(method(copy:))]
        fn copy_icon(&self, _sender: Option<&AnyObject>) {
            let Some(path) = self.ivars().paths.first().filter(|_| self.can_copy()) else {
                return NSBeep();
            };
            let board = NSPasteboard::generalPasteboard();
            let change = board.clearContents();
            if let Some(icns) = stored(path) {
                return put(&board, &icns);
            }
            if path.is_absolute() {
                return self.send(Event::Copying {
                    path: path.clone(),
                    change,
                });
            }
            if let Some(icns) = self.image().and_then(|image| in_aqua(|| icns_of(&image))) {
                put(&board, &icns);
            }
        }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            let Some(path) = self.ivars().paths.first() else {
                return NSBeep();
            };
            if !self.can_copy() || !self.can_clear() {
                return NSBeep();
            }
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            if let Some(icns) = stored(path).or_else(|| drawn(path)) {
                put(&board, &icns);
            }
            self.change(None, "Cut");
        }

        #[unsafe(method(delete:))]
        fn delete(&self, _sender: Option<&AnyObject>) {
            if !self.can_clear() {
                return NSBeep();
            }
            self.change(None, "Clear");
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            let pasted = self
                .can_paste()
                .then(|| pasted(&NSPasteboard::generalPasteboard()))
                .flatten();
            match pasted {
                Some(image) => self.change(Some(&image), "Paste"),
                None => NSBeep(),
            }
        }

        #[unsafe(method(undo:))]
        fn undo(&self, _sender: Option<&AnyObject>) {
            match self.can_paste().then(undo).flatten() {
                Some(paths) => self.send(Event::Changed(paths)),
                None => NSBeep(),
            }
        }

        #[unsafe(method(redo:))]
        fn redo(&self, _sender: Option<&AnyObject>) {
            match self.can_paste().then(redo).flatten() {
                Some(paths) => self.send(Event::Changed(paths)),
                None => NSBeep(),
            }
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.ivars().handed.set(false);
            let board = info.draggingPasteboard();
            let items = board.pasteboardItems().map_or(0, |items| items.count());
            if !self.can_change() || items != 1 {
                return NSDragOperation::None;
            }
            if first_file(&board).is_none() {
                self.ivars().handed.set(true);
                return unsafe { msg_send![super(self), draggingEntered: info] };
            }
            if info.draggingSourceOperationMask().is_empty() {
                return NSDragOperation::None;
            }
            self.highlight(true);
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            match self.ivars().handed.get() {
                true => unsafe { msg_send![super(self), draggingExited: info] },
                false => self.highlight(false),
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let dropped = self
                .can_change()
                .then(|| dropped(&info.draggingPasteboard()))
                .flatten();
            dropped.is_some_and(|image| {
                self.change(Some(&image), "Change Icon");
                true
            })
        }

        #[unsafe(method(concludeDragOperation:))]
        fn conclude_drag_operation(&self, info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            match self.ivars().handed.get() {
                true => unsafe { msg_send![super(self), concludeDragOperation: info] },
                false => self.highlight(false),
            }
        }

        #[unsafe(method(draggingEnded:))]
        fn dragging_ended(&self, _info: &ProtocolObject<dyn NSDraggingInfo>) {
            if !self.ivars().handed.get() {
                self.highlight(false);
            }
        }

        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            let shape = self.shape();
            match self.image() {
                Some(image) => image.drawInRect(shape),
                None => {
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                        shape,
                        PAGE_RADIUS,
                        PAGE_RADIUS,
                    )
                    .fill();
                }
            }
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            self.shape()
        }
    }
);

impl IconWell {
    pub(crate) fn new(mtm: MainThreadMarker, paths: Vec<PathBuf>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WellIvars {
            paths,
            same: Cell::new(true),
            shown: Cell::new(None),
            handed: Cell::new(false),
            event: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        this.setEditable(true);
        // Finder's well takes a click but stays out of the Tab loop.
        this.setRefusesFirstResponder(true);
        this.setIdentifier(Some(ns_string!("Icon")));
        this.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeURL }]));
        WELLS.with_borrow_mut(|wells| {
            wells.retain(|well| well.load().is_some());
            wells.push(Weak::new(&this));
        });
        this
    }

    /// `image` runs only when an icon changed since the last call, or `same` did.
    pub(crate) fn sync(&self, same: bool, image: impl FnOnce() -> Option<Retained<NSImage>>) {
        let ivars = self.ivars();
        ivars.same.set(same);
        let now = Some((generation(), same));
        if ivars.shown.get() != now {
            ivars.shown.set(now);
            self.setImage(image().as_deref());
            self.noteFocusRingMaskChanged();
        }
    }

    pub(crate) fn take_event(&self) -> Option<Event> {
        self.ivars().event.take()
    }

    fn send(&self, event: Event) {
        *self.ivars().event.borrow_mut() = Some(event);
        unsafe { self.sendAction_to(self.action(), self.target().as_deref()) };
    }

    fn can_change(&self) -> bool {
        let paths = &self.ivars().paths;
        !paths.is_empty() && paths.iter().all(|path| changeable(path))
    }

    fn can_copy(&self) -> bool {
        !self.ivars().paths.is_empty() && self.ivars().same.get()
    }

    fn can_clear(&self) -> bool {
        self.ivars().paths.iter().any(|path| custom(path)) && self.can_change()
    }

    fn can_paste(&self) -> bool {
        self.can_change() && holds_icon(&NSPasteboard::generalPasteboard())
    }

    fn change(&self, image: Option<&NSImage>, label: &'static str) {
        let paths = self.ivars().paths.clone();
        if apply(&paths, image, label) {
            self.send(Event::Changed(paths));
        }
    }

    fn highlight(&self, on: bool) {
        if let Some(cell) = self.cell() {
            cell.highlight_withFrame_inView(on, self.bounds(), self);
        }
    }

    fn shape(&self) -> NSRect {
        let bounds = self.bounds();
        let Some(image) = self.image() else {
            return bounds;
        };
        let size = image.size();
        if size.width <= 0. || size.height <= 0. {
            return bounds;
        }
        let fit = (bounds.size.width / size.width).min(bounds.size.height / size.height);
        let (wide, tall) = (size.width * fit, size.height * fit);
        NSRect::new(
            NSPoint::new(
                bounds.origin.x + (bounds.size.width - wide) / 2.,
                bounds.origin.y + (bounds.size.height - tall) / 2.,
            ),
            NSSize::new(wide, tall),
        )
    }
}

/// Runs in gpui after the well's AppKit action returned.
pub(crate) fn handle(event: Event, cx: &mut App) {
    match event {
        Event::Changed(paths) => refresh(&paths, cx),
        Event::Copying { path, change } => {
            cx.spawn(async move |cx| {
                let icns = cx
                    .background_executor()
                    .spawn(async move { drawn(&path) })
                    .await;
                let board = NSPasteboard::generalPasteboard();
                if let Some(icns) = icns.filter(|_| board.changeCount() == change) {
                    put(&board, &icns);
                }
            })
            .detach();
        }
    }
}

pub(crate) fn refresh(paths: &[PathBuf], cx: &mut App) {
    GENERATION.set(GENERATION.get() + 1);
    for path in paths {
        let key = SharedString::from(path.to_string_lossy().into_owned());
        crate::custom::set_custom_icon(&key, custom(path));
        for image in crate::icons::redraw(&key) {
            cx.drop_image(image, None);
        }
    }
    crate::read::notify_windows(cx);
}

/// Undo and Redo of the key window's icon well, when the well itself isn't first responder.
pub(crate) fn history() -> Option<(Option<&'static str>, Option<&'static str>)> {
    in_key_window().then(labels)
}

pub(crate) fn undo_in_key_window(cx: &mut App) {
    if let Some(paths) = in_key_window().then(undo).flatten() {
        refresh(&paths, cx);
    }
}

pub(crate) fn redo_in_key_window(cx: &mut App) {
    if let Some(paths) = in_key_window().then(redo).flatten() {
        refresh(&paths, cx);
    }
}

fn in_key_window() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(key) = NSApplication::sharedApplication(mtm).keyWindow() else {
        return false;
    };
    WELLS.with_borrow(|wells| {
        wells
            .iter()
            .filter_map(Weak::load)
            .filter_map(|well| well.window())
            .any(|window| ptr::eq(&*window, &*key))
    })
}

fn labels() -> (Option<&'static str>, Option<&'static str>) {
    let last = |stack: &RefCell<Vec<Change>>| stack.borrow().last().map(|change| change.label);
    (UNDO.with(last), REDO.with(last))
}

fn titled(verb: &str, label: Option<&str>) -> Retained<NSString> {
    NSString::from_str(&label.map_or_else(|| verb.to_owned(), |label| format!("{verb} {label}")))
}

fn undo() -> Option<Vec<PathBuf>> {
    let change = UNDO.with_borrow_mut(Vec::pop)?;
    let paths = restore(&change.before);
    REDO.with_borrow_mut(|redo| redo.push(change));
    Some(paths)
}

fn redo() -> Option<Vec<PathBuf>> {
    let change = REDO.with_borrow_mut(Vec::pop)?;
    let paths = restore(&change.after);
    UNDO.with_borrow_mut(|undo| undo.push(change));
    Some(paths)
}

fn restore(icons: &Icons) -> Vec<PathBuf> {
    let mut failed = false;
    for (path, icns) in icons {
        let image = icns.as_deref().and_then(icns_image);
        failed |= !set_icon(path, image.as_deref());
    }
    if failed {
        NSBeep();
    }
    icons.iter().map(|(path, _)| path.clone()).collect()
}

fn apply(paths: &[PathBuf], image: Option<&NSImage>, label: &'static str) -> bool {
    let before = snapshot(paths);
    let mut set = 0;
    for path in paths {
        set += usize::from(set_icon(path, image));
    }
    if set < paths.len() {
        NSBeep();
    }
    if set == 0 {
        return false;
    }
    let after = snapshot(paths);
    UNDO.with_borrow_mut(|undo| {
        undo.push(Change {
            label,
            before,
            after,
        })
    });
    REDO.with_borrow_mut(Vec::clear);
    true
}

fn snapshot(paths: &[PathBuf]) -> Icons {
    paths
        .iter()
        .map(|path| {
            let kept = custom(path).then(|| stored(path).or_else(|| drawn(path)));
            (path.clone(), kept.flatten())
        })
        .collect()
}

fn set_icon(path: &Path, image: Option<&NSImage>) -> bool {
    NSWorkspace::sharedWorkspace().setIcon_forFile_options(
        image,
        &NSString::from_str(&path.to_string_lossy()),
        NSWorkspaceIconCreationOptions::empty(),
    )
}

fn changeable(path: &Path) -> bool {
    path.is_absolute() && custom_icon::can_change(path)
}

fn custom(path: &Path) -> bool {
    path.is_absolute() && has_custom_icon(path)
}

fn stored(path: &Path) -> Option<Vec<u8>> {
    custom(path).then(|| custom_icon::stored(path)).flatten()
}

/// The icon as shown, drawn as an icns: what Finder copies of an icon that isn't custom.
fn drawn(path: &Path) -> Option<Vec<u8>> {
    let path = NSString::from_str(&path.to_string_lossy());
    in_aqua(|| icns_of(&NSWorkspace::sharedWorkspace().iconForFile(&path)))
}

fn holds_icon(board: &NSPasteboard) -> bool {
    let icns = NSString::from_str(ICNS);
    board
        .types()
        .is_some_and(|types| types.containsObject(&icns))
        || NSImage::canInitWithPasteboard(board)
}

fn first_file(board: &NSPasteboard) -> Option<Retained<NSURL>> {
    let file_url = unsafe { NSPasteboardTypeFileURL };
    board
        .pasteboardItems()?
        .iter()
        .find_map(|item| item.stringForType(file_url))
        .and_then(|url| NSURL::URLWithString(&url))
}

/// Finder's order: an image file's own picture, then icns data, then any image the pasteboard holds.
fn pasted(board: &NSPasteboard) -> Option<Retained<NSImage>> {
    let picture = first_file(board)
        .filter(|url| conforms(url, IMAGE))
        .and_then(|url| match conforms(&url, ICNS) {
            true => NSImage::initWithContentsOfURL(NSImage::alloc(), &url),
            false => {
                NSImage::initWithContentsOfURL(NSImage::alloc(), &url).and_then(|i| fitted(&i))
            }
        });
    picture
        .or_else(|| {
            board
                .dataForType(&NSString::from_str(ICNS))
                .and_then(|data| icns_image(&data.to_vec()))
        })
        .or_else(|| pictured(board))
}

/// One dragged file gives its icns, its picture or its own icon; anything else, its image.
fn dropped(board: &NSPasteboard) -> Option<Retained<NSImage>> {
    let Some(url) = first_file(board) else {
        return pictured(board);
    };
    let path = url.path()?;
    if Path::new(&path.to_string())
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("icns"))
    {
        return NSData::dataWithContentsOfURL(&url).and_then(|data| icns_image(&data.to_vec()));
    }
    conforms(&url, IMAGE)
        .then(|| NSImage::initWithContentsOfURL(NSImage::alloc(), &url).and_then(|i| fitted(&i)))
        .flatten()
        .or_else(|| Some(NSWorkspace::sharedWorkspace().iconForFile(&path)))
}

fn pictured(board: &NSPasteboard) -> Option<Retained<NSImage>> {
    NSImage::canInitWithPasteboard(board)
        .then(|| NSImage::initWithPasteboard(NSImage::alloc(), board))
        .flatten()
        .and_then(|image| fitted(&image))
}

fn conforms(url: &NSURL, identifier: &str) -> bool {
    let mut kind: Option<Retained<AnyObject>> = None;
    let _ = unsafe { url.getResourceValue_forKey_error(&mut kind, NSURLContentTypeKey) };
    let (Some(kind), Some(class)) = (kind, AnyClass::get(c"UTType")) else {
        return false;
    };
    let other: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, typeWithIdentifier: &*NSString::from_str(identifier)] };
    other.is_some_and(|other| unsafe { msg_send![&*kind, conformsToType: &*other] })
}

fn icns_image(icns: &[u8]) -> Option<Retained<NSImage>> {
    NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(icns))
}

fn fitted(image: &NSImage) -> Option<Retained<NSImage>> {
    let largest = IMAGE_SIZES[IMAGE_SIZES.len() - 1];
    let icon = NSImage::initWithSize(NSImage::alloc(), NSSize::new(largest, largest));
    for points in IMAGE_SIZES {
        for scale in SCALES {
            let rep = rendered(image, points, scale)?;
            icon.addRepresentation(&rep);
        }
    }
    Some(icon)
}

fn rendered(image: &NSImage, points: f64, scale: f64) -> Option<Retained<NSBitmapImageRep>> {
    let size = image.size();
    if size.width <= 0. || size.height <= 0. {
        return None;
    }
    let pixels = (points * scale).round() as isize;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            pixels,
            pixels,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            pixels * 4,
            32,
        )
    }?;
    rep.setSize(NSSize::new(points, points));
    let fit = points / size.width.max(size.height);
    let (wide, tall) = (size.width * fit, size.height * fit);
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(
            NSPoint::new((points - wide) / 2., (points - tall) / 2.),
            NSSize::new(wide, tall),
        ),
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    Some(rep)
}

fn icns_of(image: &NSImage) -> Option<Vec<u8>> {
    let mut reps = Vec::new();
    for points in ICNS_SIZES {
        for scale in SCALES {
            reps.push((rendered(image, points, scale)?, scale));
        }
    }
    let data = NSMutableData::new();
    let destination = unsafe {
        Retained::from_raw(CGImageDestinationCreateWithData(
            &data,
            &NSString::from_str(ICNS),
            reps.len(),
            ptr::null(),
        ))
    }?;
    for (rep, scale) in &reps {
        let image = rep.CGImage()?;
        let dpi = NSNumber::new_f64(72. * scale);
        let properties = NSDictionary::from_slices(
            unsafe { &[kCGImagePropertyDPIWidth, kCGImagePropertyDPIHeight] },
            &[&*dpi, &*dpi],
        );
        let image: *const c_void = Retained::as_ptr(&image).cast();
        unsafe { CGImageDestinationAddImage(&destination, image, &properties) };
    }
    unsafe { CGImageDestinationFinalize(&destination) }.then(|| data.to_vec())
}

/// Icons have dark renditions; Finder copies the light one.
fn in_aqua<T>(draw: impl Fn() -> T) -> T {
    let Some(aqua) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }) else {
        return draw();
    };
    let slot = RefCell::new(None);
    let block = StackBlock::new(|| *slot.borrow_mut() = Some(draw()));
    aqua.performAsCurrentDrawingAppearance(&block);
    slot.into_inner().unwrap_or_else(|| draw())
}

fn put(board: &NSPasteboard, icns: &[u8]) {
    board.setData_forType(Some(&NSData::with_bytes(icns)), &NSString::from_str(ICNS));
}

/// Whether every path shows one icon, compared as drawn; Finder looks at no more than 100.
pub(crate) fn same_icons(paths: &[PathBuf]) -> bool {
    if paths.len() > SAME_AT_MOST || !paths.iter().all(|path| path.is_absolute()) {
        return false;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let mut first: Option<Vec<u8>> = None;
    for path in paths {
        let icon = workspace.iconForFile(&NSString::from_str(&path.to_string_lossy()));
        let Some(pixels) = in_aqua(|| pixels_of(&icon)) else {
            return false;
        };
        match &first {
            Some(seen) if *seen != pixels => return false,
            Some(_) => {}
            None => first = Some(pixels),
        }
    }
    true
}

fn pixels_of(image: &NSImage) -> Option<Vec<u8>> {
    let rep = rendered(image, SAME_SIDE, 1.)?;
    let bytes = rep.bitmapData();
    if bytes.is_null() {
        return None;
    }
    let length = rep.bytesPerRow() as usize * SAME_SIDE as usize;
    Some(unsafe { std::slice::from_raw_parts(bytes, length) }.to_vec())
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGIconPromise"]
    #[ivars = PathBuf]
    struct Promise;

    impl Promise {
        #[unsafe(method(pasteboard:provideDataForType:))]
        fn provide(&self, board: &NSPasteboard, kind: &NSString) {
            if let Some(icns) = stored(self.ivars()).or_else(|| drawn(self.ivars())) {
                board.setData_forType(Some(&NSData::with_bytes(&icns)), kind);
            }
        }
    }
);

/// Finder copies a file with its icon as icns, drawn only when something reads it.
pub(crate) fn promise_icon(board: &NSPasteboard, path: &Path) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let promise = Promise::alloc(mtm).set_ivars(path.to_path_buf());
    let promise: Retained<Promise> = unsafe { msg_send![super(promise), init] };
    let types = NSArray::from_retained_slice(&[NSString::from_str(ICNS)]);
    let owner: &AnyObject = &promise;
    unsafe { board.addTypes_owner(&types, Some(owner)) };
    PROMISED.set(Some(promise));
}
