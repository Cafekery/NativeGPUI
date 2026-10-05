use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_char};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use finder_fs::flags::IS_STATIONERY;
use finder_fs::meta::Meta;
use finder_fs::permissions::{self, Grant, Privilege};
use finder_fs::tags::Tag;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, ObjectFit, Render, RenderImage,
    SharedString, Task, WeakEntity, Window, WindowBounds, WindowOptions, canvas, div, img, point,
    prelude::*, px, size,
};
use native_gpui::tokens::{FileType, SystemColor, system_color};
use native_gpui::{
    ButtonBezel, CheckState, ControlSize, LabelAlign, Layer, NativeHost, NativeLabel, NativeRead,
    Target, native_button, native_checkbox, native_label, native_root, native_rule, window_options,
    wire,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSActionCell, NSAlert, NSAlertFirstButtonReturn, NSAutoresizingMaskOptions, NSBorderType,
    NSCell, NSColor, NSControl, NSControlSize, NSControlTextEditingDelegate, NSFocusRingType,
    NSFont, NSFontAttributeName, NSFontWeightRegular, NSForegroundColorAttributeName, NSImage,
    NSImageScaling, NSImageSymbolConfiguration, NSImageView, NSLineBreakMode, NSMenu, NSMenuItem,
    NSModalResponse, NSModalResponseOK, NSOpenPanel, NSPopUpArrowPosition, NSPopUpButton,
    NSPopUpButtonCell, NSResponder, NSScrollView, NSSegmentStyle, NSSegmentSwitchTracking,
    NSSegmentedControl, NSTableCellView, NSTableColumn, NSTableHeaderCell, NSTableView,
    NSTableViewColumnAutoresizingStyle, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewGridLineStyle, NSTableViewRowSizeStyle, NSTableViewStyle, NSText, NSTextField,
    NSTextFieldCell, NSTextView, NSView, NSWindow, NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSDate, NSDateFormatter, NSDateFormatterStyle, NSDictionary,
    NSMutableAttributedString, NSNumber, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect,
    NSSize, NSString, NSURL, NSURLHasHiddenExtensionKey, NSURLIsUserImmutableKey, ns_string,
};

use crate::app::FinderDemo;
use crate::art::row_art;
use crate::data::{
    Face, Folder, Line, Row, byte_string, count, folder_of, intern_path, small_text_width,
    volume_at,
};
use crate::frame::ns_window;
use crate::prefs;
use crate::tag_editor::{tag_field, tags_in};
use crate::tags::{held_tags, retag};

unsafe extern "C" {
    fn getuid() -> u32;
    fn access(path: *const c_char, mode: i32) -> i32;
}

const W_OK: i32 = 2;
const GRANTS_PANE: f32 = 194.5;
const OWNERSHIP_ROW: f32 = 20.;
const LOCK_ICON: &str =
    "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/LockedIcon.icns";
const MANY_ICON: &str =
    "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/MultipleItemsIcon.icns";

pub(crate) const WIDTH: f32 = 265.;
pub(crate) const HEIGHT: f32 = 541.;
const TITLEBAR: f32 = 32.;
const LINE: f32 = 14.;
const SMALL: f32 = 11.;
const BODY: f32 = 24.5;
const FOOT: f32 = 8.5;
const SHUT: f32 = 20.;
const CHECK_ROW: f32 = 20.;
const TAGGED_HEAD: f32 = 88.5;
const SIMPLE_HEAD: f32 = 51.5;
const PREVIEW_ICON: f32 = 128.;
const PREVIEW_MOST: f32 = 480.;
const WIDTH_KEY: &str = "FXInfoWindowWidth";
const EXPANDED_KEY: &str = "FXInfoPanesExpanded";
/// The zero-width space lets a line start at ".config": UAX #14 never breaks before a full stop.
const CRUMB_SEPARATOR: &str = " \u{25b8} \u{200b}";
const KEPT: usize = 512;
const FOLLOW_EVERY: Duration = Duration::from_millis(100);
const FOLLOW_TRIES: usize = 600;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Pane {
    General,
    MetaData,
    Name,
    Comments,
    OpenWith,
    Preview,
    Privileges,
}

const PANES: [Pane; 7] = [
    Pane::General,
    Pane::MetaData,
    Pane::Name,
    Pane::Comments,
    Pane::OpenWith,
    Pane::Preview,
    Pane::Privileges,
];

impl Pane {
    fn key(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::MetaData => "MetaData",
            Pane::Name => "Name",
            Pane::Comments => "Comments",
            Pane::OpenWith => "OpenWith",
            Pane::Preview => "Preview",
            Pane::Privileges => "Privileges",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Pane::General => "General:",
            Pane::MetaData => "More Info:",
            Pane::Name => "Name & Extension:",
            Pane::Comments => "Comments:",
            Pane::OpenWith => "Open with:",
            Pane::Preview => "Preview:",
            Pane::Privileges => "Sharing & Permissions:",
        }
    }
}

fn panes_open() -> [bool; 7] {
    PANES.map(|pane| {
        prefs::setting_flag(EXPANDED_KEY, pane.key())
            .unwrap_or(matches!(pane, Pane::General | Pane::Preview))
    })
}

fn info_width() -> f32 {
    prefs::setting_number(WIDTH_KEY)
        .map_or(WIDTH, |width| width as f32)
        .max(WIDTH)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Subject {
    File,
    Folder,
    App,
    Volume,
    Place,
}

impl Subject {
    fn of(row: &Row) -> Self {
        if crate::locations::info_fields(row).is_some() {
            return Self::Place;
        }
        if volume_at(&row.path).is_some() {
            return Self::Volume;
        }
        match (row.face, row.package) {
            (_, true) if row.path.ends_with(".app") => Self::App,
            (Face::Folder, false) => Self::Folder,
            _ => Self::File,
        }
    }

    fn panes(self) -> &'static [Pane] {
        match self {
            Self::File => &PANES,
            Self::Place => &[Pane::General, Pane::Preview],
            _ => &[
                Pane::General,
                Pane::MetaData,
                Pane::Name,
                Pane::Comments,
                Pane::Preview,
                Pane::Privileges,
            ],
        }
    }
}

#[derive(Copy, Clone)]
struct Weight {
    bytes: u64,
    on_disk: u64,
    items: Option<u64>,
}

/// What the preview pane draws: the item's icon, a picture as tall as its aspect, or a page.
enum Shown {
    Icon,
    Picture(Arc<RenderImage>),
    Page(Arc<RenderImage>),
}

impl Shown {
    fn height(&self, width: f32) -> f32 {
        match self {
            Shown::Icon => PREVIEW_ICON,
            Shown::Page(_) => PREVIEW_MOST,
            Shown::Picture(image) => {
                let size = image.size(0);
                let aspect = u32::from(size.height) as f32 / u32::from(size.width).max(1) as f32;
                (width * aspect).round().min(PREVIEW_MOST)
            }
        }
    }
}

fn weigh_tree(path: &Path, counted: bool) -> Weight {
    let mut weight = Weight {
        bytes: 0,
        on_disk: 0,
        items: counted.then_some(0),
    };
    let mut linked = HashSet::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.nlink() > 1 && !meta.is_dir() && !linked.insert(meta.ino()) {
                continue;
            }
            weight.on_disk += meta.blocks() * 512;
            if let Some(items) = &mut weight.items {
                *items += 1;
            }
            match meta.is_dir() {
                true => pending.push(entry.path()),
                false => weight.bytes += meta.len(),
            }
        }
    }
    weight
}

fn exact_bytes(bytes: u64) -> String {
    match bytes < 1000 {
        true => byte_string(bytes),
        false => format!("{} bytes", count(bytes as usize)),
    }
}

fn items_text(items: u64) -> String {
    match items {
        1 => "1 item".to_owned(),
        _ => format!("{} items", count(items as usize)),
    }
}

fn by(width: impl std::fmt::Display, height: impl std::fmt::Display) -> SharedString {
    format!("{width}\u{200a}\u{d7}\u{200a}{height}").into()
}

/// The width an `NSTextField` label needs to show `text` whole.
fn field_width(text: &str, size: f32, bold: bool) -> f32 {
    thread_local! {
        static SEEN: RefCell<HashMap<(String, u32, bool), f32>> = RefCell::new(HashMap::new());
    }
    let key = (text.to_owned(), size.to_bits(), bold);
    if let Some(width) = SEEN.with_borrow(|seen| seen.get(&key).copied()) {
        return width;
    }
    let mtm = MainThreadMarker::new().expect("laid out on the main thread");
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font = match bold {
        true => NSFont::boldSystemFontOfSize(size.into()),
        false => NSFont::systemFontOfSize(size.into()),
    };
    field.setFont(Some(&font));
    let width = field.fittingSize().width as f32;
    SEEN.with_borrow_mut(|seen| {
        if seen.len() >= KEPT {
            seen.clear();
        }
        seen.insert(key, width);
    });
    width
}

/// How many 14 pt lines a wrapping 11 pt label `width` wide takes for `text`.
fn line_count(text: &str, width: f32) -> usize {
    thread_local! {
        static SEEN: RefCell<HashMap<(String, u32), usize>> = RefCell::new(HashMap::new());
    }
    let key = (text.to_owned(), width.to_bits());
    if let Some(lines) = SEEN.with_borrow(|seen| seen.get(&key).copied()) {
        return lines;
    }
    let mtm = MainThreadMarker::new().expect("laid out on the main thread");
    let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    field.setFont(Some(&NSFont::systemFontOfSize(SMALL.into())));
    let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(width.into(), 1e6));
    let height = field.cell().map_or(f64::from(LINE), |cell| {
        cell.cellSizeForBounds(bounds).height
    });
    let lines = ((height / f64::from(LINE)).round() as usize).max(1);
    SEEN.with_borrow_mut(|seen| {
        if seen.len() >= KEPT {
            seen.clear();
        }
        seen.insert(key, lines);
    });
    lines
}

fn formatter(date: NSDateFormatterStyle, time: NSDateFormatterStyle) -> Retained<NSDateFormatter> {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(date);
    formatter.setTimeStyle(time);
    formatter
}

fn format_date(formatter: &NSDateFormatter, seconds: f64) -> String {
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
        .to_string()
}

/// "Today, 9:54 PM" near now, else "Sep 24, 2026 at 9:10 AM".
fn header_date(seconds: f64) -> String {
    use NSDateFormatterStyle as Style;
    thread_local! {
        static DAY: Retained<NSDateFormatter> = {
            let day = formatter(Style::MediumStyle, Style::NoStyle);
            day.setDoesRelativeDateFormatting(true);
            day
        };
        static PLAIN: Retained<NSDateFormatter> = formatter(Style::MediumStyle, Style::NoStyle);
        static TIME: Retained<NSDateFormatter> = formatter(Style::NoStyle, Style::ShortStyle);
        static BOTH: Retained<NSDateFormatter> = formatter(Style::MediumStyle, Style::ShortStyle);
    }
    let day = DAY.with(|day| format_date(day, seconds));
    match PLAIN.with(|plain| format_date(plain, seconds)) == day {
        true => BOTH.with(|both| format_date(both, seconds)),
        false => format!("{day}, {}", TIME.with(|time| format_date(time, seconds))),
    }
}

/// The fullest date style that fits `room`.
fn long_date(seconds: f64, room: f32) -> String {
    use NSDateFormatterStyle as Style;
    thread_local! {
        static STYLES: [Retained<NSDateFormatter>; 4] =
            [Style::FullStyle, Style::LongStyle, Style::MediumStyle, Style::ShortStyle]
                .map(|style| formatter(style, Style::ShortStyle));
    }
    STYLES.with(|styles| {
        let mut texts = styles.iter().map(|style| format_date(style, seconds));
        let mut text = texts.next().unwrap_or_default();
        while small_text_width(&text) > room {
            let Some(shorter) = texts.next() else { break };
            text = shorter;
        }
        text
    })
}

fn app_kind(path: &Path, kind: &str) -> String {
    const ARM64: isize = 0x0100_000c;
    const X86_64: isize = 0x0100_0007;
    let architectures = path
        .to_str()
        .and_then(|path| NSBundle::bundleWithPath(&NSString::from_str(path)))
        .and_then(|bundle| bundle.executableArchitectures())
        .map(|all| {
            all.iter()
                .map(|arch| arch.integerValue())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let named = match (
        architectures.contains(&ARM64),
        architectures.contains(&X86_64),
    ) {
        (true, true) => "Universal",
        (true, false) => "Apple silicon",
        (false, true) => "Intel",
        (false, false) => return kind.to_owned(),
    };
    format!("{kind} ({named})")
}

fn system_version(mount: &str) -> Option<String> {
    let plist = Path::new(mount).join("System/Library/CoreServices/SystemVersion.plist");
    let url = crate::openwith::url(&plist)?;
    let found: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { NSDictionary::dictionaryWithContentsOfURL_error(&url) }.ok()?;
    let version = found.objectForKey(ns_string!("ProductVersion"))?;
    Some(version.downcast::<NSString>().ok()?.to_string())
}

fn set_resource(path: &Path, key: &NSString, on: bool) -> bool {
    let Some(url) = crate::openwith::url(path) else {
        return false;
    };
    unsafe { url.setResourceValue_forKey_error(Some(&NSNumber::new_bool(on)), key) }.is_ok()
}

fn resource(path: &Path, key: &NSString) -> bool {
    let Some(url) = crate::openwith::url(path) else {
        return false;
    };
    let mut value = None;
    let _ = unsafe { url.getResourceValue_forKey_error(&mut value, key) };
    value
        .and_then(|value| value.downcast::<NSNumber>().ok())
        .is_some_and(|value| value.boolValue())
}

fn writable(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { access(path.as_ptr(), W_OK) == 0 }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn titlebar_height(window: &NSWindow) -> f32 {
    (window.frame().size.height - window.contentLayoutRect().size.height) as f32
}

fn fit_window(window: &NSWindow, height: f32) {
    let frame = window.frame();
    let mut height = f64::from(height);
    if let Some(screen) = window.screen() {
        height = height.min(screen.visibleFrame().size.height);
    }
    window.setContentMinSize(NSSize::new(WIDTH.into(), height));
    window.setContentMaxSize(NSSize::new(1e4, height));
    if (frame.size.height - height).abs() < 0.25 {
        return;
    }
    let top = frame.origin.y + frame.size.height;
    window.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.origin.x, top - height),
            NSSize::new(frame.size.width, height),
        ),
        true,
    );
}

fn key_of(name: &str, path: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    (name, path).hash(&mut hasher);
    hasher.finish()
}

/// An AppKit view of the example's own, placed at the element's bounds every paint.
fn hosted<V: Message + AsRef<NSView> + 'static>(
    key: u64,
    make: impl Fn(MainThreadMarker, &Target) -> Retained<V> + 'static,
    sync: impl Fn(&V, &NativeHost) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let host = NativeHost::get(window, cx);
            let view = host.place(
                key,
                Layer::Above,
                bounds,
                window.content_mask().bounds,
                &make,
            );
            sync(&view, &host);
        },
    )
    .size_full()
}

fn icon_of(path: &str) -> Option<Retained<NSImage>> {
    let workspace = NSWorkspace::sharedWorkspace();
    if Path::new(path).is_absolute() {
        return Some(workspace.iconForFile(&NSString::from_str(path)));
    }
    match crate::locations::artwork(path)? {
        FileType::Uti(uti) => {
            let class = AnyClass::get(c"UTType")?;
            let kind: Option<Retained<AnyObject>> =
                unsafe { msg_send![class, typeWithIdentifier: &*NSString::from_str(uti)] };
            Some(unsafe { msg_send![&*workspace, iconForContentType: &*kind?] })
        }
        FileType::Named(name) => NSImage::imageNamed(&NSString::from_str(name)),
        FileType::Path(path) => Some(workspace.iconForFile(&NSString::from_str(path))),
        _ => None,
    }
}

/// The item's own icon, never its preview: Finder heads an image's info with the PNG icon.
fn icon_view(row: &Row, name: &str, side: f32, scale: f32) -> AnyElement {
    let Some(icon) = icon_of(&row.path) else {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(row_art(row, side, scale, true))
            .into_any_element();
    };
    hosted(
        key_of(name, &row.path),
        move |mtm, _| {
            let view = NSImageView::new(mtm);
            view.setImage(Some(&icon));
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view
        },
        |_, _| {},
    )
    .into_any_element()
}

fn small(id: impl Into<gpui::ElementId>, text: &str) -> NativeLabel {
    native_label(id, text).font_size(px(SMALL))
}

fn secondary(id: impl Into<gpui::ElementId>, text: &str) -> NativeLabel {
    small(id, text).color(SystemColor::SecondaryLabel)
}

/// Finder's measured frames: each pane positions its views from the top of its own band.
struct Sheet {
    width: f32,
    y: f32,
    out: Vec<AnyElement>,
}

impl Sheet {
    fn new(width: f32) -> Self {
        Sheet {
            width,
            y: 0.,
            out: Vec::new(),
        }
    }

    fn put(&mut self, (x, y): (f32, f32), (width, height): (f32, f32), child: impl IntoElement) {
        self.out.push(
            div()
                .absolute()
                .left(px(x))
                .top(px(self.y + y))
                .w(px(width))
                .h(px(height))
                .child(child)
                .into_any_element(),
        );
    }

    /// Label column right-aligned to x 68, values wrapping from x 72.
    fn rows(&mut self, rows: &[(&str, SharedString)]) -> f32 {
        let value_w = self.width - 82.;
        let mut y = BODY;
        for (at, (label, value)) in rows.iter().enumerate() {
            let height = line_count(value, value_w) as f32 * LINE;
            self.put(
                (-2., y),
                (72., LINE),
                small(("ng.finder.info.key", at), &format!("{label}:")).align(LabelAlign::Right),
            );
            self.put(
                (70., y),
                (value_w, height),
                small(("ng.finder.info.value", at), value).wraps(true),
            );
            y += height;
        }
        y
    }

    fn page(self, titlebar: f32) -> impl IntoElement {
        native_root(
            div()
                .size_full()
                .relative()
                .bg(system_color(SystemColor::WindowBackground))
                .child(
                    div()
                        .id("ng.finder.info.scroll")
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(titlebar))
                        .bottom_0()
                        .overflow_y_scroll()
                        .child(div().relative().w_full().h(px(self.y)).children(self.out)),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(titlebar))
                        .h(px(0.5))
                        .child(native_rule("ng.finder.info.titlebar")),
                ),
        )
    }
}

/// The apps that can open the item, the one it opens with, and its type's default.
struct Opener {
    apps: Vec<PathBuf>,
    chosen: Option<PathBuf>,
    default: Option<PathBuf>,
    rev: u64,
}

impl Opener {
    fn of(path: &Path, rev: u64) -> Self {
        let default = crate::openwith::type_default(path);
        let mut apps: Vec<PathBuf> = crate::openwith::apps_for(path)
            .into_iter()
            .map(|app| app.path)
            .collect();
        if let Some(at) = default
            .as_ref()
            .and_then(|default| apps.iter().position(|app| app == default))
        {
            let default = apps.remove(at);
            apps.insert(0, default);
        }
        Opener {
            chosen: crate::openwith::opener(path),
            apps,
            default,
            rev,
        }
    }

    /// The pop-up's rows, `None` being a separator: the type's default, the other apps, then the
    /// App Store and a chooser.
    fn rows(&self) -> Vec<Option<Pick>> {
        let mut rows = Vec::new();
        for (at, app) in self.apps.iter().enumerate() {
            if at == 1 && self.default.is_some() {
                rows.push(None);
            }
            rows.push(Some(Pick::App(app.clone())));
        }
        if !rows.is_empty() {
            rows.push(None);
        }
        rows.extend([Some(Pick::Store), Some(Pick::Other)]);
        rows
    }
}

#[derive(Clone, PartialEq)]
enum Pick {
    App(PathBuf),
    Store,
    Other,
}

#[derive(Copy, Clone)]
enum Check {
    Stationery,
    Locked,
}

pub(crate) struct Info {
    line: Line,
    where_: SharedString,
    subject: Subject,
    demo: WeakEntity<FinderDemo>,
    tags: Option<Rc<[Tag]>>,
    open: [bool; 7],
    weight: Option<Weight>,
    weighing: bool,
    metadata: Option<Meta>,
    fetching: bool,
    owned: bool,
    locked: bool,
    stationery: bool,
    renamable: bool,
    hidden_extension: bool,
    following: Option<Task<()>>,
    titled: String,
    opener: Option<Opener>,
    panel: bool,
    width: f32,
    asked: f32,
    preview: Shown,
    access: Option<Access>,
    first_mode: Option<u32>,
}

impl Info {
    pub(crate) fn new(line: Line, where_: SharedString, demo: WeakEntity<FinderDemo>) -> Self {
        let path = Path::new(line.row.path.as_ref());
        let subject = Subject::of(&line.row);
        let meta = std::fs::symlink_metadata(path).ok();
        let weight = match (subject, &meta) {
            (Subject::File, Some(meta)) if !meta.is_dir() => Some(Weight {
                bytes: meta.len(),
                on_disk: meta.blocks() * 512,
                items: None,
            }),
            _ => None,
        };
        let real = meta.is_some();
        let locked = real && finder_fs::ops::locked(path);
        let first_mode = real.then(|| permissions::mode(path).ok()).flatten();
        Info {
            tags: (real && subject != Subject::Place).then(|| held_tags(path).into()),
            owned: meta.is_some_and(|meta| meta.uid() == unsafe { getuid() }),
            locked,
            stationery: subject == Subject::File && finder_fs::flags::of(path) & IS_STATIONERY != 0,
            renamable: real
                && matches!(subject, Subject::File | Subject::Folder | Subject::App)
                && path.parent().is_some_and(writable),
            hidden_extension: real && resource(path, unsafe { NSURLHasHiddenExtensionKey }),
            following: None,
            titled: String::new(),
            opener: None,
            line,
            where_,
            subject,
            demo,
            open: panes_open(),
            weight,
            weighing: false,
            metadata: None,
            fetching: false,
            panel: false,
            width: info_width(),
            asked: 0.,
            preview: Shown::Icon,
            access: None,
            first_mode,
        }
    }

    pub(crate) fn in_panel(mut self) -> Self {
        self.panel = true;
        self.width = WIDTH;
        self
    }

    fn path(&self) -> &Path {
        Path::new(self.line.row.path.as_ref())
    }

    fn weigh(&mut self, cx: &mut Context<Self>) {
        let walks = matches!(self.subject, Subject::Folder | Subject::App)
            || (self.subject == Subject::File && self.line.row.package);
        if !walks || self.weight.is_some() || self.weighing {
            return;
        }
        self.weighing = true;
        let path = self.path().to_path_buf();
        let counted = self.subject == Subject::Folder;
        cx.spawn(async move |this, cx| {
            let weight = cx
                .background_executor()
                .spawn(async move { weigh_tree(&path, counted) })
                .await;
            this.update(cx, |info, cx| {
                info.weight = Some(weight);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_metadata(&mut self, cx: &mut Context<Self>) {
        let shown = self.open[Pane::MetaData as usize] && self.subject != Subject::Place;
        if !shown || self.metadata.is_some() || self.fetching {
            return;
        }
        self.fetching = true;
        let path = self.path().to_path_buf();
        cx.spawn(async move |this, cx| {
            let metadata = cx
                .background_executor()
                .spawn(async move { finder_fs::meta::meta(&path) })
                .await;
            this.update(cx, |info, cx| {
                info.metadata = Some(metadata);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let at = pane as usize;
        self.open[at] = !self.open[at];
        save_panes(self.open);
        cx.notify();
    }

    fn set_check(&mut self, check: Check, on: bool, cx: &mut Context<Self>) {
        let path = self.path().to_path_buf();
        match check {
            Check::Stationery => {
                if finder_fs::flags::set(&path, IS_STATIONERY, on).is_ok() {
                    self.stationery = on;
                }
            }
            Check::Locked => {
                if set_resource(&path, unsafe { NSURLIsUserImmutableKey }, on) {
                    self.locked = on;
                }
            }
        }
        cx.notify();
    }

    fn reload(&mut self) {
        if let Some(line) = crate::data::path_line(intern_path(self.path())) {
            self.line = line;
        }
    }

    /// The title and proxy icon follow the item through a rename.
    fn title(&mut self, window: &mut Window) {
        let title = format!("{} Info", self.line.row.display);
        if self.titled == title {
            return;
        }
        window.set_window_title(&title);
        if let Some(ns) = ns_window(window).filter(|_| !self.panel) {
            let shown = crate::openwith::url(self.path()).filter(|_| self.path().is_absolute());
            ns.setRepresentedURL(shown.as_deref());
        }
        self.titled = title;
    }

    /// Renames through the Finder window, which may first ask about the extension, then follows
    /// the item to its new name for a while.
    fn rename(&mut self, name: &str, cx: &mut Context<Self>) {
        let from = self.path().to_path_buf();
        let wanted = name.trim().replace('/', ":");
        if wanted.is_empty() || from.file_name().is_some_and(|old| *old == *wanted) {
            return;
        }
        let Some(demo) = self.demo.upgrade() else {
            return;
        };
        let key = self.line.row.path.clone();
        demo.update(cx, |demo, cx| demo.rename(&key, name, cx));
        let to = from.with_file_name(&wanted);
        let inode = std::fs::symlink_metadata(&from).ok().map(|meta| meta.ino());
        self.following = Some(cx.spawn(async move |this, cx| {
            for _ in 0..FOLLOW_TRIES {
                cx.background_executor().timer(FOLLOW_EVERY).await;
                if std::fs::symlink_metadata(&to).ok().map(|meta| meta.ino()) != inode {
                    continue;
                }
                let Some(line) = crate::data::path_line(intern_path(&to)) else {
                    return;
                };
                this.update(cx, |info, cx| {
                    info.line = line;
                    cx.notify();
                })
                .ok();
                return;
            }
        }));
    }

    fn hide_extension(&mut self, on: bool, cx: &mut Context<Self>) {
        if set_resource(self.path(), unsafe { NSURLHasHiddenExtensionKey }, on) {
            self.hidden_extension = on;
            self.reload();
        }
        cx.notify();
    }

    fn has_extension(&self) -> bool {
        matches!(self.subject, Subject::File | Subject::App)
            && self
                .path()
                .file_name()
                .and_then(|name| name.to_str()?.rsplit_once('.'))
                .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
    }

    fn head_size(&self) -> Option<String> {
        match self.subject {
            Subject::Volume | Subject::Place => None,
            _ => Some(
                self.weight
                    .map_or_else(|| "--".to_owned(), |weight| byte_string(weight.bytes)),
            ),
        }
    }

    fn general_rows(&self, room: f32) -> Vec<(&'static str, SharedString)> {
        let row = &self.line.row;
        let date = |seconds: f64| SharedString::from(long_date(seconds, room));
        match self.subject {
            Subject::Place => {
                let mut rows = crate::locations::info_fields(row).unwrap_or_default();
                if row.path == crate::locations::COMPUTER {
                    if let Some(disk) = volume_at("/") {
                        rows.push(("Startup disk", disk.name.into()));
                    }
                }
                rows
            }
            Subject::Volume => {
                let Some(volume) = volume_at(&row.path) else {
                    return Vec::new();
                };
                let used = volume.capacity.saturating_sub(volume.available);
                let mut rows = vec![
                    ("Kind", row.kind.clone()),
                    ("Created", date(row.created_at)),
                    ("Modified", date(row.modified_at)),
                ];
                if let Some(version) = system_version(&row.path) {
                    rows.push(("Version", version.into()));
                }
                rows.extend([
                    ("Format", volume.format.into()),
                    ("Capacity", byte_string(volume.capacity).into()),
                    ("Available", byte_string(volume.available).into()),
                    (
                        "Used",
                        format!("{} ({} on disk)", exact_bytes(used), byte_string(used)).into(),
                    ),
                ]);
                rows
            }
            _ => {
                let kind = match self.subject {
                    Subject::App => app_kind(self.path(), &row.kind).into(),
                    _ => row.kind.clone(),
                };
                let mut rows = vec![("Kind", kind), ("Size", weight_text(self.weight))];
                if !self.where_.is_empty() {
                    rows.push(("Where", self.where_.clone()));
                }
                rows.extend([
                    ("Created", date(row.created_at)),
                    ("Modified", date(row.modified_at)),
                ]);
                if self.subject == Subject::App {
                    if let Some(version) = finder_fs::meta::bundle_version(self.path()) {
                        rows.push(("Version", version.into()));
                    }
                }
                rows
            }
        }
    }

    /// Title, state, and the change it makes when the box can be clicked.
    fn checks(&self) -> Vec<(&'static str, bool, Option<Check>)> {
        let locked = ("Locked", self.locked, self.owned.then_some(Check::Locked));
        let shared = ("Shared folder", false, None);
        match self.subject {
            Subject::File => vec![
                (
                    "Stationery pad",
                    self.stationery,
                    self.owned.then_some(Check::Stationery),
                ),
                locked,
            ],
            Subject::Folder => vec![shared, locked],
            Subject::App => vec![locked],
            Subject::Volume => vec![shared],
            Subject::Place => Vec::new(),
        }
    }

    fn general_height(&self, width: f32) -> f32 {
        let value_w = width - 82.;
        let rows: f32 = self
            .general_rows(value_w - 4.)
            .iter()
            .map(|(_, value)| line_count(value, value_w) as f32 * LINE)
            .sum();
        let checks = match self.checks().len() {
            0 => 0.,
            checks => 8. + checks as f32 * CHECK_ROW,
        };
        BODY + rows + checks + FOOT
    }

    /// Spotlight's rows with the width of their label column, which starts at x 10.
    fn more_rows(&self, width: f32) -> (f32, Vec<(&'static str, SharedString)>) {
        let Some(metadata) = &self.metadata else {
            return (0., Vec::new());
        };
        let value = |name: &str| {
            metadata
                .values
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let mut rows = Vec::new();
        if let Some((w, h)) = metadata.pixels {
            rows.push(("Dimensions", by(w, h)));
        }
        if let Some((w, h)) =
            value("kMDItemResolutionWidthDPI").zip(value("kMDItemResolutionHeightDPI"))
        {
            rows.push(("Resolution", by(w, h)));
        }
        if let Some(duration) = metadata.duration.and_then(crate::preview::duration) {
            rows.push(("Duration", duration));
        }
        let opened = metadata.last_used;
        let label_w = rows
            .iter()
            .map(|(label, _)| *label)
            .chain(opened.map(|_| "Last opened"))
            .map(|label| field_width(&format!("{label}:"), SMALL, false))
            .fold(0., f32::max);
        if let Some(at) = opened {
            rows.push(("Last opened", long_date(at, width - 24. - label_w).into()));
        }
        (label_w, rows)
    }

    fn pane_height(&self, pane: Pane, width: f32) -> f32 {
        if !self.open[pane as usize] {
            return SHUT;
        }
        match pane {
            Pane::General => self.general_height(width),
            Pane::MetaData => BODY + self.more_rows(width).1.len().max(1) as f32 * LINE + FOOT,
            Pane::Name => 80.,
            Pane::Comments => 75.,
            Pane::OpenWith => 113.,
            Pane::Preview => BODY + self.preview.height(width - 32.) + FOOT,
            Pane::Privileges => {
                let ownership = self.access.as_ref().and_then(|access| access.ownership);
                GRANTS_PANE + ownership.map_or(0., |_| OWNERSHIP_ROW)
            }
        }
    }

    fn head_height(&self) -> f32 {
        match self.tags {
            Some(_) => TAGGED_HEAD,
            None => SIMPLE_HEAD,
        }
    }

    fn content_height(&self, width: f32) -> f32 {
        let panes: f32 = self
            .subject
            .panes()
            .iter()
            .map(|pane| self.pane_height(*pane, width))
            .sum();
        self.head_height() + panes
    }

    fn head(&self, sheet: &mut Sheet, scale: f32) {
        let row = &self.line.row;
        let width = sheet.width;
        sheet.put(
            (11., 11.),
            (32., 32.),
            icon_view(row, "ng.finder.info.icon", 32., scale),
        );
        let size = self.head_size();
        let size_w = size
            .as_deref()
            .map_or(0., |size| field_width(size, 13., false));
        if let Some(size) = &size {
            sheet.put(
                (width - 6. - size_w, 9.),
                (size_w, 16.),
                native_label("ng.finder.info.size", size)
                    .font_size(px(13.))
                    .align(LabelAlign::Right),
            );
        }
        sheet.put(
            (47., 9.),
            (width - 55. - size_w.max(17.), 16.),
            native_label("ng.finder.info.name", &row.display)
                .font_size(px(13.))
                .weight(FontWeight::BOLD),
        );
        if self.subject != Subject::Place {
            let label_w = field_width("Modified:", SMALL, false);
            sheet.put(
                (47., 28.),
                (label_w, LINE),
                secondary("ng.finder.info.modified", "Modified:"),
            );
            sheet.put(
                (47. + label_w, 28.),
                (width - 53. - label_w, LINE),
                secondary("ng.finder.info.date", &header_date(row.modified_at)),
            );
        }
        if let Some(tags) = self.tags.clone() {
            sheet.put(
                (4., 51.),
                (width - 8., 33.),
                div()
                    .size_full()
                    .border_1()
                    .border_color(system_color(SystemColor::Separator)),
            );
            sheet.put(
                (3., 52.),
                (width - 8., 31.),
                tags_field(row.path.clone(), tags),
            );
        }
        let head = self.head_height();
        sheet.put(
            (0., head - 1.),
            (width, 1.),
            native_rule("ng.finder.info.head"),
        );
        sheet.y = head;
    }

    fn pane_chrome(&self, sheet: &mut Sheet, pane: Pane, height: f32, this: &WeakEntity<Self>) {
        let this = this.clone();
        pane_chrome(
            sheet,
            pane,
            self.open[pane as usize],
            height,
            self.subject.panes().last() != Some(&pane),
            Rc::new(move |cx: &mut App| {
                this.update(cx, |info, cx| info.toggle(pane, cx)).ok();
            }),
        );
    }

    fn general(&self, sheet: &mut Sheet, this: &WeakEntity<Self>) {
        let width = sheet.width;
        let mut y = sheet.rows(&self.general_rows(width - 86.)) + 8.;
        for (at, (title, on, check)) in self.checks().into_iter().enumerate() {
            let mut box_ = native_checkbox(("ng.finder.info.check", at), on)
                .title(title)
                .control_size(ControlSize::Small)
                .enabled(check.is_some());
            if let Some(check) = check {
                let this = this.clone();
                box_ = box_.on_change(move |state: &CheckState, _, cx| {
                    this.update(cx, |info, cx| {
                        info.set_check(check, *state == CheckState::On, cx)
                    })
                    .ok();
                });
            }
            sheet.put((55., y + 6.), (width - 65., 16.), box_);
            y += CHECK_ROW;
        }
    }

    fn more_info(&self, sheet: &mut Sheet) {
        let width = sheet.width;
        let (label_w, rows) = self.more_rows(width);
        if rows.is_empty() {
            if self.metadata.is_some() {
                sheet.put(
                    (10., BODY),
                    (30., LINE),
                    secondary("ng.finder.info.more.none", "--"),
                );
            }
            return;
        }
        for (at, (label, value)) in rows.iter().enumerate() {
            let y = BODY + at as f32 * LINE;
            sheet.put(
                (10., y),
                (label_w, LINE),
                small(("ng.finder.info.more.key", at), &format!("{label}:"))
                    .align(LabelAlign::Right),
            );
            sheet.put(
                (10. + label_w, y),
                (width - 20. - label_w, LINE),
                small(("ng.finder.info.more.value", at), value),
            );
        }
    }

    fn name_pane(&self, sheet: &mut Sheet, this: &WeakEntity<Self>) {
        let width = sheet.width;
        let renamable = self.renamable && !self.locked;
        sheet.put(
            (11., BODY - 1.),
            (width - 22., 26.),
            name_field(self.line.row.path.clone(), renamable, this.clone()),
        );
        let this = this.clone();
        sheet.put(
            (16., 57.5),
            (100., 18.),
            native_checkbox("ng.finder.info.hide", self.hidden_extension)
                .title("Hide extension")
                .control_size(ControlSize::Small)
                .enabled(renamable && self.has_extension())
                .on_change(move |state: &CheckState, _, cx| {
                    this.update(cx, |info, cx| {
                        info.hide_extension(*state == CheckState::On, cx)
                    })
                    .ok();
                }),
        );
    }

    fn comments(&self, sheet: &mut Sheet) {
        let editable = self.renamable && !self.locked && writable(self.path());
        sheet.put(
            (4., BODY),
            (sheet.width - 8., 46.),
            comment_box(self.line.row.path.clone(), editable),
        );
    }

    fn ensure_access(&mut self) {
        let shown = self.open[Pane::Privileges as usize] && self.subject != Subject::Place;
        if shown && self.access.is_none() {
            self.access = Some(Access::of(self.path(), self.subject));
        }
    }

    /// Only the owner may change a class's privilege without root; an ACL entry has no class.
    fn grant(&mut self, row: usize, privilege: Privilege, cx: &mut Context<Self>) {
        let class = self
            .access
            .as_ref()
            .and_then(|access| access.grants.get(row)?.class);
        if let Some(class) = class {
            permissions::set(self.path(), class, privilege).ok();
        }
        self.access = Some(Access::of(self.path(), self.subject));
        cx.notify();
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        if let Some(mode) = self.first_mode {
            permissions::set_mode(self.path(), mode).ok();
        }
        self.access = Some(Access::of(self.path(), self.subject));
        cx.notify();
    }

    fn confirm_apply(&self, window: Option<Retained<NSWindow>>, cx: &mut Context<Self>) {
        let (Some(window), Some(mtm)) = (window, MainThreadMarker::new()) else {
            return;
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(ns_string!(
            "Are you sure you want to apply the selected owner, group, and permissions to all the enclosed items?"
        ));
        alert.setInformativeText(ns_string!("You can\u{2019}t undo this."));
        alert.addButtonWithTitle(ns_string!("OK"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        let path = self.path().to_path_buf();
        let background = cx.background_executor().clone();
        let handler = RcBlock::new(move |response: NSModalResponse| {
            if response == NSAlertFirstButtonReturn {
                let path = path.clone();
                background
                    .spawn(async move { permissions::apply_to_enclosed(&path) })
                    .detach();
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
    }

    fn privileges(&self, sheet: &mut Sheet, this: &WeakEntity<Self>) {
        let Some(access) = &self.access else {
            return;
        };
        let width = sheet.width;
        let path = self.line.row.path.clone();
        let folder = matches!(self.subject, Subject::Folder | Subject::Volume);
        sheet.put(
            (12., BODY),
            (width - 24., LINE),
            small("ng.finder.info.yours", yours_text(access.yours)),
        );
        sheet.put(
            (14., 46.5),
            (width - 29., 113.),
            grants_table(
                path.clone(),
                &access.grants,
                self.owned,
                folder,
                this.clone(),
            ),
        );
        let changed = access.mode.is_some() && access.mode != self.first_mode;
        sheet.put(
            (15., 162.5),
            (tools_width(), 22.),
            grant_tools(path, self.owned, folder, changed, this.clone()),
        );
        let bottom = self.pane_height(Pane::Privileges, width);
        if let Some(ignores) = access.ownership {
            sheet.put(
                (14., GRANTS_PANE - 7.),
                (191., 16.),
                native_checkbox("ng.finder.info.ownership", ignores)
                    .title("Ignore ownership on this volume")
                    .control_size(ControlSize::Small)
                    .enabled(false),
            );
        }
        sheet.put((width - 32., bottom - 27.), (18., 18.), lock_view());
    }

    /// A picture or page once Quick Look has one; until then, and for what it can't preview, the
    /// icon.
    fn shown(&self, scale: f32) -> Shown {
        let row = &self.line.row;
        if self.subject != Subject::File || !self.open[Pane::Preview as usize] {
            return Shown::Icon;
        }
        match crate::thumbs::thumb(row, PREVIEW_MOST, scale, true, false) {
            Some(image) if matches!(row.face, Face::Image | Face::Movie) => Shown::Picture(image),
            Some(image) => Shown::Page(image),
            None => Shown::Icon,
        }
    }

    fn preview_pane(&self, sheet: &mut Sheet, scale: f32) {
        let width = sheet.width - 32.;
        let height = self.preview.height(width);
        match &self.preview {
            Shown::Icon => sheet.put(
                (16., BODY),
                (width, PREVIEW_ICON),
                icon_view(
                    &self.line.row,
                    "ng.finder.info.preview",
                    PREVIEW_ICON,
                    scale,
                ),
            ),
            Shown::Picture(image) => sheet.put(
                (16., BODY),
                (width, height),
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            ),
            Shown::Page(image) => {
                let size = image.size(0);
                let aspect = u32::from(size.height) as f32 / u32::from(size.width).max(1) as f32;
                sheet.put(
                    (15.5, BODY - 0.5),
                    (width + 1., height + 1.),
                    div()
                        .size_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(system_color(SystemColor::Separator))
                        .bg(system_color(SystemColor::TextBackground))
                        .overflow_hidden()
                        .child(
                            img(image.clone())
                                .w(px(width - 1.))
                                .h(px((width - 1.) * aspect)),
                        ),
                );
            }
        }
    }

    fn ensure_opener(&mut self) {
        let shown = self.open[Pane::OpenWith as usize] && self.subject == Subject::File;
        if shown && self.opener.is_none() {
            self.opener = Some(Opener::of(self.path(), 0));
        }
    }

    fn open_with(&self, sheet: &mut Sheet, this: &WeakEntity<Self>) {
        let Some(opener) = &self.opener else {
            return;
        };
        let width = sheet.width;
        sheet.put(
            (15., 23.5),
            (width - 30., 22.),
            apps_popup(self.line.row.path.clone(), opener, this.clone()),
        );
        sheet.put(
            (13., 48.5),
            (width - 26., 2. * LINE),
            small(
                "ng.finder.info.openwith.note",
                "Use this application to open all documents like this one.",
            )
            .wraps(true),
        );
        let differs = opener.chosen.is_some() && opener.chosen != opener.default;
        let this = this.clone();
        sheet.put(
            (15., 84.5),
            (86., 20.),
            native_button("ng.finder.info.change.all", "Change All\u{2026}")
                .control_size(ControlSize::Small)
                .enabled(differs)
                .on_click(move |_, window, cx| {
                    let ns = ns_window(window);
                    this.update(cx, |info, cx| info.change_all(ns, cx)).ok();
                }),
        );
    }

    /// An app sets this item's own choice; the App Store and Other… rows act and leave it.
    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        let path = self.path().to_path_buf();
        let Some(opener) = &mut self.opener else {
            return;
        };
        match opener.rows().into_iter().nth(row).flatten() {
            Some(Pick::App(app)) => {
                crate::openwith::set_default(&[path], &app);
                opener.chosen = Some(app);
            }
            Some(Pick::Store) => crate::openwith::app_store(&[path]),
            Some(Pick::Other) => choose_other(path, cx),
            None => {}
        }
        cx.notify();
    }

    fn change_all(&mut self, window: Option<Retained<NSWindow>>, cx: &mut Context<Self>) {
        let app = self
            .opener
            .as_ref()
            .and_then(|opener| opener.chosen.clone());
        let (Some(app), Some(window), Some(mtm)) = (app, window, MainThreadMarker::new()) else {
            return;
        };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Are you sure you want to change all similar documents to open with the application \u{201c}{}\u{201d}?",
            finder_fs::name::of_path(&app)
        )));
        let detail = match self.path().extension() {
            Some(extension) => format!(
                "This change will apply to all documents with extension \u{201c}{}\u{201d}.",
                extension.to_string_lossy()
            ),
            None => format!(
                "This change will apply to all documents with type \u{201c}{}\u{201d}.",
                self.line.row.kind
            ),
        };
        alert.setInformativeText(&NSString::from_str(&detail));
        alert.addButtonWithTitle(ns_string!("Continue"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        let this = cx.entity().downgrade();
        let async_cx = cx.to_async();
        // `RcBlock`: AppKit holds the handler until the sheet ends, long after this returns.
        let handler = RcBlock::new(move |response: NSModalResponse| {
            if response == NSAlertFirstButtonReturn {
                let mut cx = async_cx.clone();
                this.update(&mut cx, |info, cx| info.make_default(&app, cx))
                    .ok();
            }
        });
        alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
    }

    fn make_default(&mut self, app: &Path, cx: &mut Context<Self>) {
        crate::openwith::set_type_default(self.path(), app);
        if let Some(opener) = &mut self.opener {
            opener.apps.retain(|known| known != app);
            opener.apps.insert(0, app.to_path_buf());
            opener.default = Some(app.to_path_buf());
            opener.rev += 1;
        }
        cx.notify();
    }
}

fn tags_field(path: SharedString, tags: Rc<[Tag]>) -> impl IntoElement {
    let key = key_of("ng.finder.info.tags", &path);
    hosted(
        key,
        move |mtm, target| {
            let field = tag_field(mtm, NSRect::ZERO, &tags);
            field.setFont(Some(&NSFont::systemFontOfSize(12.)));
            field.setBezeled(false);
            field.setBordered(false);
            field.setDrawsBackground(false);
            field.setFocusRingType(NSFocusRingType::None);
            field.setPlaceholderString(Some(ns_string!("Add Tags…")));
            if let Some(cell) = field.cell() {
                cell.setSendsActionOnEndEditing(true);
            }
            wire(&field, target, key);
            field
        },
        move |_, host| {
            let read: NativeRead = Rc::new(|sender: &AnyObject| -> Box<dyn Any> {
                Box::new(
                    sender
                        .downcast_ref::<NSTextField>()
                        .map(tags_in)
                        .unwrap_or_default(),
                )
            });
            let path = path.clone();
            host.on_event(
                key,
                read,
                Some(Rc::new(move |value, _, cx| {
                    let Ok(tags) = value.downcast::<Vec<Tag>>() else {
                        return;
                    };
                    retag(
                        vec![PathBuf::from(path.as_ref())],
                        move |paths| {
                            let held = finder_fs::tags::of(&paths[0]);
                            if !held
                                .iter()
                                .map(|tag| &tag.name)
                                .eq(tags.iter().map(|tag| &tag.name))
                            {
                                finder_fs::tags::set(&paths[0], &tags).ok();
                            }
                        },
                        cx,
                    );
                })),
            );
        },
    )
}

/// Renames on Return or when it loses focus, as Finder's field does. The field's bezel draws a
/// point outside its frame, so it sits inset in a holder the host can clip.
fn name_field(path: SharedString, enabled: bool, this: WeakEntity<Info>) -> impl IntoElement {
    let key = key_of("ng.finder.info.name.field", &path);
    let name = Path::new(path.as_ref())
        .file_name()
        .map(|name| finder_fs::name::display(&name.to_string_lossy()).into_owned())
        .unwrap_or_default();
    hosted(
        key,
        move |mtm, target| {
            let holder = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 100., 26.));
            let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(1., 1., 98., 24.));
            field.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            field.setFont(Some(&NSFont::systemFontOfSize(13.)));
            field.setStringValue(&NSString::from_str(&name));
            if let Some(cell) = field.cell() {
                cell.setSendsActionOnEndEditing(true);
                cell.setScrollable(true);
            }
            wire(&field, target, key);
            holder.addSubview(&field);
            holder
        },
        move |holder, host| {
            let field = holder.subviews().firstObject();
            if let Some(field) = field.and_then(|field| field.downcast::<NSTextField>().ok()) {
                if field.isEnabled() != enabled {
                    field.setEnabled(enabled);
                }
            }
            let read: NativeRead = Rc::new(|sender: &AnyObject| -> Box<dyn Any> {
                Box::new(
                    sender
                        .downcast_ref::<NSTextField>()
                        .map(|field| field.stringValue().to_string())
                        .unwrap_or_default(),
                )
            });
            let this = this.clone();
            host.on_event(
                key,
                read,
                Some(Rc::new(move |value, _, cx| {
                    if let Ok(name) = value.downcast::<String>() {
                        this.update(cx, |info, cx| info.rename(&name, cx)).ok();
                    }
                })),
            );
        },
    )
}

struct Access {
    grants: Vec<Grant>,
    yours: Privilege,
    mode: Option<u32>,
    ownership: Option<bool>,
}

impl Access {
    /// The startup disk shows no "Ignore ownership" row.
    fn of(path: &Path, subject: Subject) -> Self {
        Access {
            grants: permissions::grants(path).unwrap_or_default(),
            yours: permissions::yours(path),
            mode: permissions::mode(path).ok(),
            ownership: (subject == Subject::Volume && path != Path::new("/"))
                .then(|| finder_fs::volumes::ignores_ownership(path)),
        }
    }
}

fn yours_text(privilege: Privilege) -> &'static str {
    match privilege {
        Privilege::ReadWrite => "You can read and write",
        Privilege::ReadOnly => "You can only read",
        Privilege::WriteOnly => "You can only write (Drop Box)",
        Privilege::NoAccess => "You have no access",
        Privilege::Custom => "You have custom access",
    }
}

const PRIVILEGES: [Privilege; 5] = [
    Privilege::ReadWrite,
    Privilege::ReadOnly,
    Privilege::WriteOnly,
    Privilege::NoAccess,
    Privilege::Custom,
];

fn privilege_title(privilege: Privilege) -> &'static str {
    match privilege {
        Privilege::ReadWrite => "Read & Write",
        Privilege::ReadOnly => "Read only",
        Privilege::WriteOnly => "Write only (Drop Box)",
        Privilege::NoAccess => "No Access",
        Privilege::Custom => "Custom",
    }
}

fn privilege_tag(privilege: Privilege) -> isize {
    PRIVILEGES
        .iter()
        .position(|known| *known == privilege)
        .unwrap_or(0) as isize
}

fn row_tag(path: &str, row: usize) -> u64 {
    key_of("ng.finder.info.grant", &format!("{path}\u{0}{row}"))
}

struct GrantsIvars {
    cells: Vec<[Retained<NSView>; 2]>,
    fitted: Cell<f64>,
}

define_class!(
    /// Finder indents its column titles past AppKit's own inset.
    #[unsafe(super(NSTableHeaderCell, NSTextFieldCell, NSActionCell, NSCell, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoGrantsHeader"]
    #[ivars = f64]
    struct GrantsHeader;

    impl GrantsHeader {
        #[unsafe(method(drawInteriorWithFrame:inView:))]
        fn draw_interior(&self, frame: NSRect, view: &NSView) {
            let indent = *self.ivars();
            let frame = rect(
                frame.origin.x + indent,
                frame.origin.y,
                frame.size.width - indent,
                frame.size.height,
            );
            unsafe { msg_send![super(self), drawInteriorWithFrame: frame, inView: view] }
        }
    }
);

impl GrantsHeader {
    fn new(mtm: MainThreadMarker, title: &str, indent: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(indent);
        unsafe { msg_send![super(this), initTextCell: &*NSString::from_str(title)] }
    }
}

define_class!(
    /// Disabled, it takes no clicks yet keeps its rows' colours, as Finder's does.
    #[unsafe(super(NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoGrants"]
    #[ivars = GrantsIvars]
    struct Grants;

    impl Grants {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            match self.isEnabled() {
                true => unsafe { msg_send![super(self), hitTest: point] },
                false => None,
            }
        }
    }

    unsafe impl NSObjectProtocol for Grants {}

    unsafe impl NSTableViewDataSource for Grants {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _: &NSTableView) -> isize {
            self.ivars().cells.len() as isize
        }
    }

    unsafe impl NSControlTextEditingDelegate for Grants {}

    unsafe impl NSTableViewDelegate for Grants {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(
            &self,
            _: &NSTableView,
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            let at = usize::from(column.is_some_and(|column| &*column.identifier() == ns_string!("priv")));
            let cells = usize::try_from(row)
                .ok()
                .and_then(|row| self.ivars().cells.get(row));
            cells.map(|cells| cells[at].clone())
        }
    }
);

fn name_cell(mtm: MainThreadMarker, grant: &Grant) -> Retained<NSView> {
    let cell = NSTableCellView::initWithFrame(NSTableCellView::alloc(mtm), rect(0., 0., 113., 14.));
    let symbol = match grant.group {
        true => ns_string!("person.2.fill"),
        false => ns_string!("person.crop.circle"),
    };
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(symbol, None) {
        let size = image.size();
        let icon = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            rect(
                15.5 - size.width / 2.,
                (14. - size.height) / 2.,
                size.width,
                size.height,
            ),
        );
        icon.setImage(Some(&image));
        icon.setImageScaling(NSImageScaling::ScaleNone);
        icon.setContentTintColor(Some(&NSColor::labelColor()));
        cell.addSubview(&icon);
        unsafe { cell.setImageView(Some(&icon)) };
    }
    let title = match grant.me {
        true => format!("{} (Me)", grant.name),
        false => grant.name.clone(),
    };
    let label = NSTextField::labelWithString(&NSString::from_str(&title), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(SMALL.into())));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label.setFrame(rect(28., 0., 85., 14.));
    label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    cell.addSubview(&label);
    unsafe { cell.setTextField(Some(&label)) };
    Retained::into_super(cell)
}

/// Write only is a folder's choice; a row already holding another keeps it listed.
fn privilege_popup(
    mtm: MainThreadMarker,
    privilege: Privilege,
    folder: bool,
) -> Retained<NSPopUpButton> {
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    popup.setBordered(false);
    popup.setControlSize(NSControlSize::Small);
    popup.setFont(Some(&NSFont::systemFontOfSize(SMALL.into())));
    if let Some(cell) = popup
        .cell()
        .and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
    {
        cell.setArrowPosition(NSPopUpArrowPosition::ArrowAtBottom);
    }
    for choice in PRIVILEGES {
        let listed = match choice {
            Privilege::WriteOnly => folder,
            Privilege::Custom => false,
            _ => true,
        };
        if !listed && choice != privilege {
            continue;
        }
        popup.addItemWithTitle(&NSString::from_str(privilege_title(choice)));
        if let Some(item) = popup.lastItem() {
            item.setTag(privilege_tag(choice));
        }
    }
    popup
}

/// Finder's table: owner, ACL entries, group and everyone, each with its privilege.
fn grants_table(
    path: SharedString,
    grants: &[Grant],
    editable: bool,
    folder: bool,
    this: WeakEntity<Info>,
) -> impl IntoElement {
    let rows: String = grants
        .iter()
        .map(|grant| {
            format!(
                "{}\u{0}{:?}\u{0}{:?}\u{0}",
                grant.name, grant.class, grant.privilege
            )
        })
        .collect();
    let key = key_of(
        "ng.finder.info.grants",
        &format!("{path}\u{0}{folder}\u{0}{rows}"),
    );
    let made = grants.to_vec();
    let states: Vec<(isize, bool)> = grants
        .iter()
        .map(|grant| (privilege_tag(grant.privilege), grant.class.is_some()))
        .collect();
    let tags_path = path.clone();
    hosted(
        key,
        move |mtm, target| {
            let scroll =
                NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0., 0., 236., 113.));
            scroll.setBorderType(NSBorderType::BezelBorder);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            let cells = made
                .iter()
                .enumerate()
                .map(|(row, grant)| {
                    let popup = privilege_popup(mtm, grant.privilege, folder);
                    wire(&popup, target, row_tag(&path, row));
                    popup.setFrame(rect(-1., 0., 111., 14.));
                    let holder = NSTableCellView::initWithFrame(
                        NSTableCellView::alloc(mtm),
                        rect(0., 0., 112., 14.),
                    );
                    holder.addSubview(&popup);
                    [name_cell(mtm, grant), Retained::into_super(holder)]
                })
                .collect();
            let table = Grants::alloc(mtm).set_ivars(GrantsIvars {
                cells,
                fitted: Cell::new(0.),
            });
            let table: Retained<Grants> =
                unsafe { msg_send![super(table), initWithFrame: NSRect::ZERO] };
            table.setStyle(NSTableViewStyle::Plain);
            table.setRowSizeStyle(NSTableViewRowSizeStyle::Custom);
            table.setRowHeight(LINE.into());
            table.setIntercellSpacing(NSSize::new(3., 2.));
            table.setGridStyleMask(NSTableViewGridLineStyle::SolidVerticalGridLineMask);
            table.setUsesAlternatingRowBackgroundColors(true);
            table.setColumnAutoresizingStyle(
                NSTableViewColumnAutoresizingStyle::UniformColumnAutoresizingStyle,
            );
            table.setAllowsColumnReordering(false);
            table.setFocusRingType(NSFocusRingType::None);
            for (id, title, indent) in [
                (ns_string!("name"), "Name", 5.),
                (ns_string!("priv"), "Privilege", 13.),
            ] {
                let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), id);
                column.setHeaderCell(&GrantsHeader::new(mtm, title, indent));
                column.setWidth(108.);
                column.setMinWidth(40.);
                table.addTableColumn(&column);
            }
            unsafe {
                table.setDataSource(Some(ProtocolObject::from_ref(&*table)));
                table.setDelegate(Some(ProtocolObject::from_ref(&*table)));
            }
            scroll.setDocumentView(Some(&table));
            scroll
        },
        move |scroll, host| {
            let Some(table) = scroll
                .documentView()
                .and_then(|table| table.downcast::<Grants>().ok())
            else {
                return;
            };
            if table.isEnabled() != editable {
                table.setEnabled(editable);
            }
            let inner = scroll.contentSize().width;
            if table.ivars().fitted.get() != inner {
                table.ivars().fitted.set(inner);
                let columns = table.tableColumns();
                let half = ((inner - 6.) / 2.).round();
                for (column, width) in columns.iter().zip([half, inner - 6. - half]) {
                    column.setWidth(width);
                }
            }
            for (row, (cells, &(tag, posix))) in table.ivars().cells.iter().zip(&states).enumerate()
            {
                let popup = cells[1].subviews().firstObject();
                let Some(popup) = popup.and_then(|popup| popup.downcast::<NSPopUpButton>().ok())
                else {
                    continue;
                };
                if popup.selectedTag() != tag {
                    popup.selectItemWithTag(tag);
                }
                if popup.isEnabled() != posix {
                    popup.setEnabled(posix);
                }
                let read: NativeRead = Rc::new(|sender: &AnyObject| -> Box<dyn Any> {
                    Box::new(
                        sender
                            .downcast_ref::<NSPopUpButton>()
                            .map_or(-1, |popup| popup.selectedTag()),
                    )
                });
                let this = this.clone();
                host.on_event_channel(
                    key,
                    row_tag(&tags_path, row),
                    read,
                    Some(Rc::new(move |value, _, cx| {
                        let chosen = value
                            .downcast::<isize>()
                            .ok()
                            .and_then(|tag| PRIVILEGES.get(usize::try_from(*tag).ok()?).copied());
                        if let Some(privilege) = chosen {
                            this.update(cx, |info, cx| info.grant(row, privilege, cx))
                                .ok();
                        }
                    })),
                );
            }
        },
    )
}

fn action_item(
    mtm: MainThreadMarker,
    target: &Target,
    title: &str,
    tag: u64,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setTag(tag as isize);
    unsafe {
        item.setTarget(Some(target.as_ref()));
        item.setAction(Some(sel!(ngAction:)));
    }
    item
}

/// Add and Remove stay off: adding a user needs a directory-service picker, and only an ACL entry
/// could be removed.
fn tools_control(mtm: MainThreadMarker) -> Retained<NSSegmentedControl> {
    let control = NSSegmentedControl::new(mtm);
    control.setSegmentStyle(NSSegmentStyle::Rounded);
    control.setTrackingMode(NSSegmentSwitchTracking::Momentary);
    control.setControlSize(NSControlSize::Small);
    control.setSegmentCount(3);
    for (at, (symbol, tip)) in [
        (ns_string!("plus"), ns_string!("Add")),
        (ns_string!("minus"), ns_string!("Remove")),
        (ns_string!("ellipsis.circle"), ns_string!("Action")),
    ]
    .into_iter()
    .enumerate()
    {
        let config =
            NSImageSymbolConfiguration::configurationWithPointSize_weight(SMALL.into(), unsafe {
                NSFontWeightRegular
            });
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(symbol, Some(tip))
            .and_then(|image| image.imageWithSymbolConfiguration(&config));
        control.setImage_forSegment(image.as_deref(), at as isize);
        control.setToolTip_forSegment(Some(tip), at as isize);
        control.setEnabled_forSegment(false, at as isize);
    }
    control.setMenu_forSegment(Some(&NSMenu::new(mtm)), 2);
    control.setShowsMenuIndicator_forSegment(true, 2);
    control
}

fn tools_width() -> f32 {
    thread_local! {
        static WIDTH: f32 = MainThreadMarker::new()
            .map_or(78.5, |mtm| tools_control(mtm).intrinsicContentSize().width as f32);
    }
    WIDTH.with(|width| *width)
}

fn grant_tools(
    path: SharedString,
    editable: bool,
    folder: bool,
    changed: bool,
    this: WeakEntity<Info>,
) -> impl IntoElement {
    let key = key_of(
        "ng.finder.info.grants.tools",
        &format!("{path}\u{0}{folder}"),
    );
    let apply = key_of("ng.finder.info.grants.apply", &path);
    let revert = key_of("ng.finder.info.grants.revert", &path);
    hosted(
        key,
        move |mtm, target| {
            let control = tools_control(mtm);
            if let Some(menu) = control.menuForSegment(2) {
                menu.setAutoenablesItems(false);
                if folder {
                    menu.addItem(&action_item(
                        mtm,
                        target,
                        "Apply to enclosed items\u{2026}",
                        apply,
                    ));
                }
                menu.addItem(&action_item(mtm, target, "Revert changes", revert));
            }
            control
        },
        move |control, host| {
            if control.isEnabledForSegment(2) != editable {
                control.setEnabled_forSegment(editable, 2);
            }
            if let Some(revert_item) = control
                .menuForSegment(2)
                .and_then(|menu| menu.itemWithTag(revert as isize))
            {
                if revert_item.isEnabled() != changed {
                    revert_item.setEnabled(changed);
                }
            }
            let read: NativeRead = Rc::new(|_: &AnyObject| -> Box<dyn Any> { Box::new(()) });
            let to_revert = this.clone();
            host.on_event_channel(
                key,
                revert,
                read.clone(),
                Some(Rc::new(move |_, _, cx| {
                    to_revert.update(cx, |info, cx| info.revert(cx)).ok();
                })),
            );
            let to_apply = this.clone();
            host.on_event_channel(
                key,
                apply,
                read,
                Some(Rc::new(move |_, window, cx| {
                    let ns = ns_window(window);
                    to_apply
                        .update(cx, |info, cx| info.confirm_apply(ns, cx))
                        .ok();
                })),
            );
        },
    )
}

fn lock_view() -> impl IntoElement {
    hosted(
        key_of("ng.finder.info.lock", LOCK_ICON),
        |mtm, _| {
            let view = NSImageView::new(mtm);
            let image =
                NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(LOCK_ICON));
            view.setImage(image.as_deref());
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view
        },
        |_, _| {},
    )
}

struct CommentIvars {
    path: PathBuf,
    saved: RefCell<String>,
}

define_class!(
    /// Saves the comment where Finder keeps it when editing ends or the view leaves its window.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGInfoComment"]
    #[ivars = CommentIvars]
    struct Comment;

    impl Comment {
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if resigned {
                self.save();
            }
            resigned
        }

        #[unsafe(method(viewWillMoveToWindow:))]
        fn view_will_move_to_window(&self, window: Option<&NSWindow>) {
            if window.is_none() {
                self.save();
            }
            unsafe { msg_send![super(self), viewWillMoveToWindow: window] }
        }
    }
);

impl Comment {
    fn new(mtm: MainThreadMarker, path: PathBuf, frame: NSRect) -> Retained<Self> {
        let comment = finder_fs::comment::of(&path);
        let this = Self::alloc(mtm).set_ivars(CommentIvars {
            path,
            saved: RefCell::new(comment.clone()),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setString(&NSString::from_str(&comment));
        this
    }

    fn save(&self) {
        let text = self.string().to_string();
        let ivars = self.ivars();
        if *ivars.saved.borrow() != text && finder_fs::comment::set(&ivars.path, &text).is_ok() {
            *ivars.saved.borrow_mut() = text;
        }
    }
}

fn comment_box(path: SharedString, editable: bool) -> impl IntoElement {
    hosted(
        key_of("ng.finder.info.comment", &path),
        move |mtm, _| {
            let scroll =
                NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0., 0., 100., 46.));
            scroll.setBorderType(NSBorderType::BezelBorder);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            let size = scroll.contentSize();
            let text = Comment::new(
                mtm,
                PathBuf::from(path.as_ref()),
                rect(0., 0., size.width, size.height),
            );
            text.setMinSize(NSSize::new(0., size.height));
            text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
            text.setVerticallyResizable(true);
            text.setHorizontallyResizable(false);
            text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            if let Some(container) = unsafe { text.textContainer() } {
                container.setWidthTracksTextView(true);
            }
            text.setRichText(false);
            text.setAllowsUndo(true);
            text.setFont(Some(&NSFont::systemFontOfSize(SMALL.into())));
            text.setTextColor(Some(&NSColor::labelColor()));
            scroll.setDocumentView(Some(&text));
            scroll
        },
        move |scroll, _| {
            let text = scroll.documentView();
            if let Some(text) = text.and_then(|text| text.downcast::<NSTextView>().ok()) {
                if text.isEditable() != editable {
                    text.setEditable(editable);
                }
            }
        },
    )
}

/// Other… chooses any app for this item, from a task: the modal loop would re-enter GPUI's borrow.
fn choose_other(path: PathBuf, cx: &mut Context<Info>) {
    cx.spawn(async move |this, cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(ns_string!("/Applications"))));
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(app) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        let app = PathBuf::from(app.to_string());
        crate::openwith::set_default(std::slice::from_ref(&path), &app);
        this.update(cx, |info, cx| {
            let rev = info.opener.as_ref().map_or(0, |opener| opener.rev + 1);
            let mut opener = Opener::of(&path, rev);
            if !opener.apps.contains(&app) {
                opener.apps.push(app.clone());
            }
            opener.chosen = Some(app);
            info.opener = Some(opener);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

fn apps_popup(path: SharedString, opener: &Opener, this: WeakEntity<Info>) -> impl IntoElement {
    let key = key_of(
        "ng.finder.info.openwith",
        &format!("{path}\u{0}{}", opener.rev),
    );
    let rows = opener.rows();
    let chosen = opener.chosen.clone().and_then(|app| {
        let app = Some(Pick::App(app));
        rows.iter().position(|row| *row == app)
    });
    let default = opener.default.clone();
    hosted(
        key,
        move |mtm, target| {
            let popup = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                NSRect::ZERO,
                false,
            );
            popup.setControlSize(NSControlSize::Small);
            popup.setFont(Some(&NSFont::systemFontOfSize(SMALL.into())));
            if let Some(menu) = popup.menu() {
                for row in &rows {
                    menu.addItem(&menu_row(mtm, row.as_ref(), default.as_deref()));
                }
            }
            wire(&popup, target, key);
            popup
        },
        move |popup, host| {
            if let Some(chosen) = chosen {
                if popup.indexOfSelectedItem() != chosen as isize {
                    popup.selectItemAtIndex(chosen as isize);
                }
            }
            let read: NativeRead = Rc::new(|sender: &AnyObject| -> Box<dyn Any> {
                Box::new(
                    sender
                        .downcast_ref::<NSPopUpButton>()
                        .map_or(-1, |popup| popup.indexOfSelectedItem()),
                )
            });
            let this = this.clone();
            host.on_event(
                key,
                read,
                Some(Rc::new(move |value, _, cx| {
                    let row = value.downcast::<isize>().ok().map(|row| *row);
                    if let Some(row) = row.and_then(|row| usize::try_from(row).ok()) {
                        this.update(cx, |info, cx| info.pick(row, cx)).ok();
                    }
                })),
            );
        },
    )
}

/// A 16 pt icon and the bundle's name, the type's default marked in the secondary colour.
fn menu_row(
    mtm: MainThreadMarker,
    row: Option<&Pick>,
    default: Option<&Path>,
) -> Retained<NSMenuItem> {
    let Some(pick) = row else {
        return NSMenuItem::separatorItem(mtm);
    };
    let item = NSMenuItem::new(mtm);
    let app = match pick {
        Pick::App(app) => app,
        Pick::Store => {
            item.setTitle(ns_string!("App Store\u{2026}"));
            return item;
        }
        Pick::Other => {
            item.setTitle(ns_string!("Other\u{2026}"));
            return item;
        }
    };
    let name = app.file_name().unwrap_or_default().to_string_lossy();
    let suffix = match Some(app.as_path()) == default {
        true => " (default)",
        false => "",
    };
    item.setTitle(&NSString::from_str(&name));
    let text =
        NSMutableAttributedString::from_nsstring(&NSString::from_str(&format!("{name}{suffix}")));
    let font = NSFont::systemFontOfSize(SMALL.into());
    let tail = NSRange::new(name.encode_utf16().count(), suffix.encode_utf16().count());
    unsafe {
        text.addAttribute_value_range(NSFontAttributeName, &font, NSRange::new(0, text.length()));
        text.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &NSColor::labelColor(),
            NSRange::new(0, text.length()),
        );
        text.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &NSColor::secondaryLabelColor(),
            tail,
        );
    }
    item.setAttributedTitle(Some(&text));
    let icon =
        NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(&app.to_string_lossy()));
    icon.setSize(NSSize::new(16., 16.));
    item.setImage(Some(&icon));
    item
}

impl Render for Info {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        if !self.panel && (width - self.width).abs() >= 0.5 {
            prefs::set_setting_number(WIDTH_KEY, width.into());
        }
        self.width = width;
        self.weigh(cx);
        self.fetch_metadata(cx);
        self.ensure_opener();
        self.preview = self.shown(window.scale_factor());
        self.ensure_access();
        cx.defer(|_| crate::thumbs::collect_thumbnails());
        self.title(window);
        let titlebar = ns_window(window).map_or(TITLEBAR, |window| titlebar_height(&window));
        let this = cx.entity().downgrade();
        let mut sheet = Sheet::new(width);
        self.head(&mut sheet, window.scale_factor());
        for &pane in self.subject.panes() {
            let height = self.pane_height(pane, width);
            self.pane_chrome(&mut sheet, pane, height, &this);
            if self.open[pane as usize] {
                match pane {
                    Pane::General => self.general(&mut sheet, &this),
                    Pane::MetaData => self.more_info(&mut sheet),
                    Pane::Name => self.name_pane(&mut sheet, &this),
                    Pane::Comments => self.comments(&mut sheet),
                    Pane::OpenWith => self.open_with(&mut sheet, &this),
                    Pane::Preview => self.preview_pane(&mut sheet, window.scale_factor()),
                    Pane::Privileges => self.privileges(&mut sheet, &this),
                }
            }
            sheet.y += height;
        }
        fit(&mut self.asked, window, titlebar + sheet.y, cx);
        sheet.page(titlebar)
    }
}

fn crumb_text(folder: &Folder) -> String {
    let names: Vec<&str> = folder.crumbs.iter().map(|&(name, _)| name).collect();
    names.join(CRUMB_SEPARATOR)
}

pub(crate) fn where_of(id: &str) -> Option<String> {
    let parent = Path::new(id)
        .parent()
        .filter(|parent| parent.is_absolute())?;
    Some(crumb_text(&folder_of(intern_path(parent))))
}

pub(crate) fn open(
    demo: &Entity<crate::app::FinderDemo>,
    at: Option<&'static str>,
    summary: bool,
    cx: &mut App,
) {
    let (lines, where_) = {
        let state = demo.read(cx);
        let chosen = match at {
            Some(_) => Vec::new(),
            None => state.chosen_lines(),
        };
        let at = at.or_else(|| (chosen.is_empty() && !state.results()).then(|| state.here()));
        let lines = match at {
            Some(id) => crate::locations::info_line(id)
                .or_else(|| crate::data::path_line(id))
                .into_iter()
                .collect(),
            None => chosen,
        };
        let where_ = at
            .and_then(where_of)
            .or_else(|| state.folder().map(crumb_text));
        (lines, SharedString::from(where_.unwrap_or_default()))
    };
    if summary && lines.len() > 1 {
        open_combined(lines, where_, cx);
        return;
    }
    for (step, line) in lines.into_iter().enumerate() {
        open_info(Info::new(line, where_.clone(), demo.downgrade()), step, cx);
    }
}

fn open_info(info: Info, step: usize, cx: &mut App) {
    let width = info.width;
    let height = TITLEBAR + info.content_height(width);
    let offset = step as f32 * 24.;
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60. + offset), px(80. + offset)),
                size: size(px(width), px(height)),
            })),
            ..window_options()
        },
        move |window, cx| {
            crate::chrome::toolbar::titlebar_corners(window);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|_| info)
        },
    );
}

pub(crate) struct Combined {
    lines: Vec<Line>,
    where_: SharedString,
    weight: Option<Weight>,
    weighing: bool,
    open: bool,
    asked: f32,
}

impl Combined {
    pub(crate) fn new(lines: Vec<Line>, where_: SharedString) -> Self {
        Combined {
            lines,
            where_,
            weight: None,
            weighing: false,
            open: panes_open()[Pane::General as usize],
            asked: 0.,
        }
    }

    /// "for 6 items" counts the selected items and everything inside them.
    fn weigh(&mut self, cx: &mut Context<Self>) {
        if self.weight.is_some() || self.weighing {
            return;
        }
        self.weighing = true;
        let paths: Vec<PathBuf> = self
            .lines
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect();
        cx.spawn(async move |this, cx| {
            let weight = cx
                .background_executor()
                .spawn(async move {
                    let mut total = Weight {
                        bytes: 0,
                        on_disk: 0,
                        items: Some(paths.len() as u64),
                    };
                    for path in &paths {
                        let Ok(meta) = std::fs::symlink_metadata(path) else {
                            continue;
                        };
                        let inner = match meta.is_dir() {
                            true => weigh_tree(path, true),
                            false => Weight {
                                bytes: meta.len(),
                                on_disk: meta.blocks() * 512,
                                items: None,
                            },
                        };
                        total.bytes += inner.bytes;
                        total.on_disk += inner.on_disk;
                        total.items = total.items.map(|items| items + inner.items.unwrap_or(0));
                    }
                    total
                })
                .await;
            this.update(cx, |combined, cx| {
                combined.weight = Some(weight);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        let mut open = panes_open();
        open[Pane::General as usize] = self.open;
        save_panes(open);
        cx.notify();
    }

    fn rows(&self) -> Vec<(&'static str, SharedString)> {
        let mut kinds: Vec<&str> = self
            .lines
            .iter()
            .map(|line| line.row.kind.as_ref())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut rows = vec![
            ("Kind", kinds.join(", ").into()),
            ("Size", weight_text(self.weight)),
        ];
        if !self.where_.is_empty() {
            rows.push(("Where", self.where_.clone()));
        }
        rows
    }
}

impl Render for Combined {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        self.weigh(cx);
        let titlebar = ns_window(window).map_or(TITLEBAR, |window| titlebar_height(&window));
        let mut sheet = Sheet::new(width);
        sheet.put((11., 11.), (32., 32.), many_icon());
        let size = self
            .weight
            .map_or_else(|| "--".to_owned(), |weight| byte_string(weight.bytes));
        let size_w = field_width(&size, 13., false);
        sheet.put(
            (width - 6. - size_w, 9.),
            (size_w, 16.),
            native_label("ng.finder.info.size", &size)
                .font_size(px(13.))
                .align(LabelAlign::Right),
        );
        sheet.put(
            (47., 9.),
            (width - 55. - size_w.max(17.), 16.),
            native_label(
                "ng.finder.info.name",
                &format!("{} items", count(self.lines.len())),
            )
            .font_size(px(13.))
            .weight(FontWeight::BOLD),
        );
        sheet.put(
            (0., SIMPLE_HEAD - 1.),
            (width, 1.),
            native_rule("ng.finder.info.head"),
        );
        sheet.y = SIMPLE_HEAD;
        let rows = self.rows();
        let height = match self.open {
            true => {
                let value_w = width - 82.;
                let lines: f32 = rows
                    .iter()
                    .map(|(_, value)| line_count(value, value_w) as f32 * LINE)
                    .sum();
                BODY + lines + FOOT
            }
            false => SHUT,
        };
        let this = cx.entity().downgrade();
        pane_chrome(
            &mut sheet,
            Pane::General,
            self.open,
            height,
            false,
            Rc::new(move |cx: &mut App| {
                this.update(cx, |combined, cx| combined.toggle(cx)).ok();
            }),
        );
        if self.open {
            sheet.rows(&rows);
        }
        sheet.y += height;
        fit(&mut self.asked, window, titlebar + sheet.y, cx);
        sheet.page(titlebar)
    }
}

fn many_icon() -> impl IntoElement {
    hosted(
        key_of("ng.finder.info.many", MANY_ICON),
        |mtm, _| {
            let view = NSImageView::new(mtm);
            let image =
                NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(MANY_ICON));
            view.setImage(image.as_deref());
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view
        },
        |_, _| {},
    )
}

fn open_combined(lines: Vec<Line>, where_: SharedString, cx: &mut App) {
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60.), px(80.)),
                size: size(px(info_width()), px(HEIGHT)),
            })),
            ..window_options()
        },
        move |window, cx| {
            window.set_window_title("Multiple Item Info");
            crate::chrome::toolbar::titlebar_corners(window);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
            }
            cx.new(|_| Combined::new(lines, where_))
        },
    );
}

fn save_panes(open: [bool; 7]) {
    let flags: Vec<(&str, bool)> = PANES
        .iter()
        .zip(open)
        .map(|(pane, open)| (pane.key(), open))
        .collect();
    prefs::set_setting_flags(EXPANDED_KEY, &flags);
}

fn weight_text(weight: Option<Weight>) -> SharedString {
    let Some(weight) = weight else {
        return "Calculating\u{2026}".into();
    };
    let text = format!(
        "{} ({} on disk)",
        exact_bytes(weight.bytes),
        byte_string(weight.on_disk)
    );
    match weight.items {
        Some(items) => format!("{text} for {}", items_text(items)).into(),
        None => text.into(),
    }
}

fn fit<T: 'static>(asked: &mut f32, window: &Window, height: f32, cx: &mut Context<T>) {
    if (height - *asked).abs() < 0.25 {
        return;
    }
    *asked = height;
    let Some(ns) = ns_window(window) else {
        return;
    };
    // Outside gpui's borrow of the window: AppKit reports the resize back synchronously.
    cx.spawn(async move |_, _| fit_window(&ns, height)).detach();
}

/// A section's header row, disclosure, title and closing rule.
fn pane_chrome(
    sheet: &mut Sheet,
    pane: Pane,
    open: bool,
    height: f32,
    rule: bool,
    flip: Rc<dyn Fn(&mut App)>,
) {
    let at = pane as usize;
    let width = sheet.width;
    let click = flip.clone();
    sheet.put(
        (0., 0.),
        (width, 16.),
        div()
            .id(("ng.finder.info.pane", at))
            .size_full()
            .on_click(move |_, _, cx| click(cx)),
    );
    sheet.put(
        (7., 3.5),
        (13., 13.),
        native_button(("ng.finder.info.disclose", at), "")
            .bezel_style(ButtonBezel::Disclosure)
            .on(open)
            .square(13.)
            .on_click(move |_, _, cx| flip(cx)),
    );
    sheet.put(
        (20., 3.5),
        (width - 40., LINE),
        small(("ng.finder.info.pane.title", at), pane.title()),
    );
    if rule {
        sheet.put(
            (0., height - 1.),
            (width, 1.),
            native_rule(("ng.finder.info.pane.rule", at)),
        );
    }
}
