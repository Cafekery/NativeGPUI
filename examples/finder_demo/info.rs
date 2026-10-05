use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use finder_fs::flags::IS_STATIONERY;
use finder_fs::tags::Tag;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, Render, SharedString, WeakEntity, Window,
    WindowBounds, WindowOptions, canvas, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{FileType, SystemColor, system_color};
use native_gpui::{
    ButtonBezel, CheckState, ControlSize, LabelAlign, Layer, NativeHost, NativeLabel, NativeRead,
    Target, native_button, native_checkbox, native_label, native_root, native_rule, window_options,
    wire,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{Message, msg_send};
use objc2_app_kit::{
    NSFocusRingType, NSFont, NSImage, NSImageScaling, NSImageView, NSTextField, NSView, NSWindow,
    NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSDate, NSDateFormatter, NSDateFormatterStyle, NSDictionary,
    NSNumber, NSPoint, NSRect, NSSize, NSString, NSURLIsUserImmutableKey, ns_string,
};

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
}

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
const WIDTH_KEY: &str = "FXInfoWindowWidth";
const EXPANDED_KEY: &str = "FXInfoPanesExpanded";
/// The zero-width space lets a line start at ".config": UAX #14 never breaks before a full stop.
const CRUMB_SEPARATOR: &str = " \u{25b8} \u{200b}";
const KEPT: usize = 512;

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
fn head_icon(row: &Row, scale: f32) -> AnyElement {
    let Some(icon) = icon_of(&row.path) else {
        return row_art(row, 32., scale, true);
    };
    hosted(
        key_of("ng.finder.info.icon", &row.path),
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

#[derive(Copy, Clone)]
enum Check {
    Stationery,
    Locked,
}

pub(crate) struct Info {
    line: Line,
    where_: SharedString,
    subject: Subject,
    tags: Option<Rc<[Tag]>>,
    open: [bool; 7],
    weight: Option<Weight>,
    weighing: bool,
    owned: bool,
    locked: bool,
    stationery: bool,
    panel: bool,
    width: f32,
    asked: f32,
}

impl Info {
    pub(crate) fn new(line: Line, where_: SharedString) -> Self {
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
        Info {
            tags: (real && subject != Subject::Place).then(|| held_tags(path).into()),
            owned: meta.is_some_and(|meta| meta.uid() == unsafe { getuid() }),
            locked: real && finder_fs::ops::locked(path),
            stationery: subject == Subject::File && finder_fs::flags::of(path) & IS_STATIONERY != 0,
            line,
            where_,
            subject,
            open: panes_open(),
            weight,
            weighing: false,
            panel: false,
            width: info_width(),
            asked: 0.,
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

    fn toggle(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let at = pane as usize;
        self.open[at] = !self.open[at];
        let flags: Vec<(&str, bool)> = PANES
            .iter()
            .zip(self.open)
            .map(|(pane, open)| (pane.key(), open))
            .collect();
        prefs::set_setting_flags(EXPANDED_KEY, &flags);
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

    fn head_size(&self) -> Option<String> {
        match self.subject {
            Subject::Volume | Subject::Place => None,
            _ => Some(
                self.weight
                    .map_or_else(|| "--".to_owned(), |weight| byte_string(weight.bytes)),
            ),
        }
    }

    fn size_text(&self) -> SharedString {
        let Some(weight) = self.weight else {
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
                let mut rows = vec![("Kind", kind), ("Size", self.size_text())];
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

    fn pane_height(&self, pane: Pane, width: f32) -> f32 {
        if !self.open[pane as usize] {
            return SHUT;
        }
        match pane {
            Pane::General => self.general_height(width),
            Pane::MetaData => BODY + LINE + FOOT,
            Pane::Name => 80.,
            Pane::Comments => 75.,
            Pane::OpenWith => 113.,
            Pane::Preview => BODY + 128. + FOOT,
            Pane::Privileges => 191.,
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

    fn fit(&mut self, window: &Window, height: f32, cx: &mut Context<Self>) {
        if (height - self.asked).abs() < 0.25 {
            return;
        }
        self.asked = height;
        let Some(ns) = ns_window(window) else {
            return;
        };
        // Outside gpui's borrow of the window: AppKit reports the resize back synchronously.
        cx.spawn(async move |_, _| fit_window(&ns, height)).detach();
    }

    fn head(&self, sheet: &mut Sheet, scale: f32) {
        let row = &self.line.row;
        let width = sheet.width;
        sheet.put((11., 11.), (32., 32.), head_icon(row, scale));
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
        let at = pane as usize;
        let width = sheet.width;
        let flip = {
            let this = this.clone();
            move |cx: &mut App| {
                this.update(cx, |info, cx| info.toggle(pane, cx)).ok();
            }
        };
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
                .on(self.open[at])
                .square(13.)
                .on_click(move |_, _, cx| flip(cx)),
        );
        sheet.put(
            (20., 3.5),
            (width - 40., LINE),
            small(("ng.finder.info.pane.title", at), pane.title()),
        );
        sheet.put(
            (0., height - 1.),
            (width, 1.),
            native_rule(("ng.finder.info.pane.rule", at)),
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

impl Render for Info {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        if !self.panel && (width - self.width).abs() >= 0.5 {
            prefs::set_setting_number(WIDTH_KEY, width.into());
        }
        self.width = width;
        self.weigh(cx);
        cx.defer(|_| crate::thumbs::collect_thumbnails());
        let titlebar = ns_window(window).map_or(TITLEBAR, |window| titlebar_height(&window));
        let this = cx.entity().downgrade();
        let mut sheet = Sheet::new(width);
        self.head(&mut sheet, window.scale_factor());
        for &pane in self.subject.panes() {
            let height = self.pane_height(pane, width);
            self.pane_chrome(&mut sheet, pane, height, &this);
            if pane == Pane::General && self.open[pane as usize] {
                self.general(&mut sheet, &this);
            }
            sheet.y += height;
        }
        self.fit(window, titlebar + sheet.y, cx);
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
        open_info(Info::new(line, where_.clone()), step, cx);
    }
}

fn open_info(info: Info, step: usize, cx: &mut App) {
    let width = info.width;
    let height = TITLEBAR + info.content_height(width);
    let title = format!("{} Info", info.line.row.display);
    let shown = crate::openwith::url(info.path()).filter(|_| info.path().is_absolute());
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
            window.set_window_title(&title);
            crate::chrome::toolbar::titlebar_corners(window);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
                ns.setRepresentedURL(shown.as_deref());
            }
            cx.new(|_| info)
        },
    );
}

pub(crate) struct Combined {
    pub(crate) lines: Vec<Line>,
    pub(crate) where_: SharedString,
}

impl Render for Combined {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        let scale = window.scale_factor();
        let titlebar = ns_window(window).map_or(TITLEBAR, |window| titlebar_height(&window));
        let total: u64 = self.lines.iter().map(|line| line.row.bytes).sum();
        let mut kinds: Vec<&str> = self
            .lines
            .iter()
            .map(|line| line.row.kind.as_ref())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut sheet = Sheet::new(width);
        sheet.put(
            (47., 9.),
            (width - 55., 16.),
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
        sheet.put(
            (20., 3.5),
            (width - 40., LINE),
            small("ng.finder.info.pane.title", Pane::General.title()),
        );
        let rows = sheet.rows(&[
            ("Kind", kinds.join(", ").into()),
            ("Size", byte_string(total).into()),
            ("Where", self.where_.clone()),
        ]);
        sheet.y += rows + FOOT;
        let listed = self.lines.len() as f32 * 20.;
        sheet.put(
            (12., 0.),
            (width - 24., listed),
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(self.lines.iter().enumerate().map(|(at, line)| {
                    div()
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(row_art(&line.row, 16., scale, false))
                        .child(
                            div()
                                .flex_1()
                                .h(px(LINE))
                                .child(small(("ng.finder.info.item", at), &line.row.display)),
                        )
                        .child(
                            div().w(px(64.)).h(px(LINE)).child(
                                secondary(("ng.finder.info.item.size", at), &line.row.size)
                                    .align(LabelAlign::Right),
                            ),
                        )
                })),
        );
        sheet.y += listed + FOOT;
        sheet.page(titlebar)
    }
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
            cx.new(|_| Combined { lines, where_ })
        },
    );
}
