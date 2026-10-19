use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{CString, c_char};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use block2::{RcBlock, StackBlock};
use finder_fs::flags::IS_STATIONERY;
use finder_fs::meta::Meta;
use finder_fs::permissions::{self, Grant, Privilege};
use finder_fs::tags::Tag;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, Render, RenderImage, SharedString, Task,
    WeakEntity, Window, WindowBounds, WindowOptions, canvas, div, img, point, prelude::*, px, size,
};
use native_gpui::tokens::{FileType, SystemColor, system_color};
use native_gpui::{
    LabelAlign, Layer, MenuEntry, NativeHost, Target, native_button, native_label, native_popup,
    window_options, wire,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, DowncastTarget, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAutoresizingMaskOptions, NSButton, NSColor, NSControlSize,
    NSFocusRingType, NSFont, NSFontWeightRegular, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem, NSModalResponse, NSPathControl,
    NSPathControlItem, NSResponder, NSScrollView, NSSegmentStyle, NSSegmentSwitchTracking,
    NSSegmentedControl, NSText, NSTextAlignment, NSTextField, NSTextView, NSView, NSWindow,
    NSWindowButton, NSWindowDidResizeNotification, NSWindowTitleVisibility, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSBundle, NSBundleExecutableArchitectureARM64,
    NSBundleExecutableArchitectureX86_64, NSDate, NSDateFormatter, NSDateFormatterStyle,
    NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSObject, NSPoint, NSRect,
    NSSize, NSString, NSURLHasHiddenExtensionKey, NSURLIsUserImmutableKey, ns_string,
};
use parking_lot::Mutex;

use crate::app::FinderDemo;
use crate::art::{row_art, small_glyph};
use crate::data::{
    Face, Line, Row, byte_string, count, folder_of, icloud_drive, intern, intern_path, volume_at,
};
use crate::fold::{self, Fold, Part};
use crate::form::{
    BODY, BOTTOM, Flip, Form, GAP, INSET, KEPT, LINE, MARGIN, ROW, SECTION, STACK, TITLE, TITLEBAR,
    choose_file, hugging, key_of, labelled, lines_height, page, remembered, resize_later,
    switch_row, text_height, text_width,
};
use crate::frame::ns_window;
use crate::icon_well::{self, IconWell};
use crate::prefs;
use crate::tag_editor::{tag_field, tags_in};
use crate::tags::{held_tags, retag};

unsafe extern "C" {
    fn getuid() -> u32;
    fn access(path: *const c_char, mode: i32) -> i32;
}

const W_OK: i32 = 2;
const LOCK_ICON: &str =
    "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/LockedIcon.icns";
const MANY_ICON: &str =
    "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/MultipleItemsIcon.icns";

pub(crate) const WIDTH: f32 = 360.;
pub(crate) const HEIGHT: f32 = 620.;
const SHORTEST: f32 = 240.;
const NARROWEST: f32 = 265.;
const WIDEST: f32 = 400.;
const TOP: f32 = 16.;
const PREVIEW: f32 = 128.;
const THUMB: f32 = 320.;
const NAME: f32 = 15.;
const COMMENT: f32 = 48.;
// NSPathControl ends its last title 2pt further in than a label ends its text.
const PATH_TRAIL: f32 = 2.;
const NOTE: &str = "Use this application to open all documents like this one.";
const WIDTH_KEY: &str = "FXInfoWindowWidth";
const EXPANDED_KEY: &str = "FXInfoPanesExpanded";
const WHERE: &str = "Where";
const FOLLOW_EVERY: Duration = Duration::from_millis(100);
const FOLLOW_TRIES: usize = 600;
const WEIGH_EVERY: Duration = Duration::from_millis(250);

fn info_width() -> f32 {
    prefs::setting_number(WIDTH_KEY)
        .map_or(WIDTH, |width| width as f32)
        .clamp(NARROWEST, WIDEST)
}

#[derive(Copy, Clone)]
enum Pane {
    General,
    MetaData,
    Name,
    Comments,
    OpenWith,
    Privileges,
}

const PANES: [Pane; 6] = [
    Pane::General,
    Pane::MetaData,
    Pane::Name,
    Pane::Comments,
    Pane::OpenWith,
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
            Pane::Privileges => "Privileges",
        }
    }

    fn section(self) -> (&'static str, &'static str) {
        match self {
            Pane::General => ("ng.finder.info.general", "General"),
            Pane::MetaData => ("ng.finder.info.more", "More Info"),
            Pane::Name => ("ng.finder.info.name", "Name & Extension"),
            Pane::Comments => ("ng.finder.info.comments", "Comments"),
            Pane::OpenWith => ("ng.finder.info.openwith", "Open with"),
            Pane::Privileges => ("ng.finder.info.privileges", "Sharing & Permissions"),
        }
    }
}

fn panes_open() -> [bool; 6] {
    PANES.map(|pane| prefs::setting_flag(EXPANDED_KEY, pane.key()).unwrap_or(true))
}

fn remember(pane: Pane, open: bool) {
    let mut kept = panes_open();
    kept[pane as usize] = open;
    let flags = PANES
        .into_iter()
        .zip(kept)
        .map(|(pane, open)| (pane.key(), open));
    prefs::set_setting_flags(EXPANDED_KEY, flags);
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
}

#[derive(Copy, Clone, Default)]
struct Weight {
    bytes: u64,
    on_disk: u64,
    items: Option<u64>,
}

impl Weight {
    fn file(meta: &std::fs::Metadata) -> Self {
        Weight {
            bytes: meta.len(),
            on_disk: meta.blocks() * 512,
            items: None,
        }
    }
}

type Report<'a> = &'a mut dyn FnMut(&Weight) -> bool;

fn weigh_folder(path: &Path, counted: bool, report: Report) -> Weight {
    let mut linked = HashSet::new();
    let Some(totals) = crate::fsearch::totals(path) else {
        let mut weight = Weight {
            items: counted.then_some(0),
            ..Weight::default()
        };
        weigh_tree(path, &mut weight, &mut linked, report);
        return weight;
    };
    let mut weight = Weight {
        bytes: totals.bytes,
        on_disk: totals.allocated,
        items: counted.then_some(totals.items),
    };
    if totals.hardlinks > 0 {
        for (ino, bytes, on_disk) in linked_files(&crate::fsearch::hardlinks(path)) {
            if linked.insert(ino) {
                continue;
            }
            weight.bytes = weight.bytes.saturating_sub(bytes);
            weight.on_disk = weight.on_disk.saturating_sub(on_disk);
            if let Some(items) = &mut weight.items {
                *items = items.saturating_sub(1);
            }
        }
    }
    for dir in &totals.unindexed {
        weigh_tree(dir, &mut weight, &mut linked, report);
    }
    weight
}

fn linked_files(paths: &[PathBuf]) -> Vec<(u64, u64, u64)> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|s| {
        let tasks: Vec<_> = paths
            .chunks(paths.len().div_ceil(threads).max(1))
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|path| std::fs::symlink_metadata(path).ok())
                        .filter(|meta| meta.nlink() > 1 && !meta.is_dir())
                        .map(|meta| (meta.ino(), meta.len(), meta.blocks() * 512))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        tasks.into_iter().flat_map(|t| t.join().unwrap()).collect()
    })
}

fn weigh_tree(path: &Path, weight: &mut Weight, linked: &mut HashSet<u64>, report: Report) {
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !report(weight) {
                return;
            }
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
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                weight.bytes += meta.len();
            }
        }
    }
}

fn weigh_all(paths: &[PathBuf], report: Report) -> Weight {
    let mut total = Weight::default();
    let mut items = paths.len() as u64;
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        let inner = if meta.is_dir() {
            weigh_folder(path, true, &mut |inner| {
                report(&Weight {
                    bytes: total.bytes + inner.bytes,
                    on_disk: total.on_disk + inner.on_disk,
                    items: Some(items + inner.items.unwrap_or(0)),
                })
            })
        } else {
            Weight::file(&meta)
        };
        total.bytes += inner.bytes;
        total.on_disk += inner.on_disk;
        items += inner.items.unwrap_or(0);
    }
    Weight {
        items: Some(items),
        ..total
    }
}

fn items_text(items: u64) -> String {
    match items {
        1 => "1 item".to_owned(),
        items => format!("{} items", count(items as usize)),
    }
}

fn weight_rows(
    weight: Option<Weight>,
    so_far: Option<Weight>,
) -> Vec<(&'static str, SharedString)> {
    let Some(shown) = weight.or(so_far) else {
        return vec![("Size", "Calculating\u{2026}".into())];
    };
    let size = match weight {
        Some(_) => size_text(shown.bytes),
        None => format!("Calculating\u{2026} {}", byte_string(shown.bytes)),
    };
    let mut rows = vec![
        ("Size", size.into()),
        ("On disk", byte_string(shown.on_disk).into()),
    ];
    rows.extend(
        shown
            .items
            .map(|items| ("Contains", items_text(items).into())),
    );
    rows
}

fn size_text(bytes: u64) -> String {
    if bytes < 1000 {
        byte_string(bytes)
    } else {
        format!("{} ({} bytes)", byte_string(bytes), count(bytes as usize))
    }
}

fn by(width: impl std::fmt::Display, height: impl std::fmt::Display) -> SharedString {
    format!("{width}\u{200a}\u{d7}\u{200a}{height}").into()
}

enum Shown {
    Icon,
    Picture(Arc<RenderImage>),
    Page(Arc<RenderImage>),
}

fn aspect(image: &RenderImage) -> f32 {
    let size = image.size(0);
    u32::from(size.height) as f32 / u32::from(size.width).max(1) as f32
}

fn long_date(seconds: f64, room: f32) -> String {
    use NSDateFormatterStyle as Style;
    thread_local! {
        static STYLES: [Retained<NSDateFormatter>; 4] =
            [Style::FullStyle, Style::LongStyle, Style::MediumStyle, Style::ShortStyle].map(
                |style| {
                    let formatter = NSDateFormatter::new();
                    formatter.setDateStyle(style);
                    formatter.setTimeStyle(Style::ShortStyle);
                    formatter
                },
            );
    }
    let date = NSDate::dateWithTimeIntervalSinceReferenceDate(seconds);
    STYLES.with(|styles| {
        let mut texts = styles
            .iter()
            .map(|style| style.stringFromDate(&date).to_string());
        let mut text = texts.next().unwrap_or_default();
        while text_width(&text) > room {
            let Some(shorter) = texts.next() else { break };
            text = shorter;
        }
        text
    })
}

fn app_kind(path: &Path, kind: &str) -> String {
    let architectures = path
        .to_str()
        .and_then(|path| NSBundle::bundleWithPath(&NSString::from_str(path)))
        .and_then(|bundle| bundle.executableArchitectures());
    let has = |wanted: u32| {
        architectures
            .as_ref()
            .is_some_and(|all| all.iter().any(|arch| arch.unsignedIntValue() == wanted))
    };
    let named = match (
        has(NSBundleExecutableArchitectureARM64),
        has(NSBundleExecutableArchitectureX86_64),
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

pub(crate) fn resource(path: &Path, key: &NSString) -> bool {
    let Some(url) = crate::openwith::url(path) else {
        return false;
    };
    let mut value = None;
    let _ = unsafe { url.getResourceValue_forKey_error(&mut value, key) };
    value
        .and_then(|value| value.downcast::<NSNumber>().ok())
        .is_some_and(|value| value.boolValue())
}

fn set_resource(path: &Path, key: &NSString, on: bool) -> bool {
    let Some(url) = crate::openwith::url(path) else {
        return false;
    };
    unsafe { url.setResourceValue_forKey_error(Some(&NSNumber::new_bool(on)), key) }.is_ok()
}

fn writable(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { access(path.as_ptr(), W_OK) == 0 }
}

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

fn on_action<V: DowncastTarget, T: 'static>(
    host: &NativeHost,
    owner: u64,
    tag: u64,
    read: impl Fn(&V) -> T + 'static,
    handle: impl Fn(T, &mut Window, &mut App) + 'static,
) {
    host.on_event_channel(
        owner,
        tag,
        Rc::new(move |sender: &AnyObject| -> Box<dyn Any> {
            Box::new(sender.downcast_ref::<V>().map(&read))
        }),
        Some(Rc::new(
            move |value: Box<dyn Any>, window: &mut Window, cx: &mut App| {
                if let Some(value) = value.downcast::<Option<T>>().ok().and_then(|value| *value) {
                    handle(value, window, cx);
                }
            },
        )),
    );
}

fn in_background<V: 'static, R: Send + 'static>(
    cx: &mut Context<V>,
    work: impl FnOnce() -> R + Send + 'static,
    done: impl FnOnce(&mut V, R) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async move { work() }).await;
        this.update(cx, |view, cx| {
            done(view, result);
            cx.notify();
        })
    })
    .detach();
}

/// Shows a running total, and stops walking once the window that asked has closed.
fn weigh_in_background<V: 'static>(
    cx: &mut Context<V>,
    work: impl FnOnce(Report) -> Weight + Send + 'static,
    show: impl Fn(&mut V, Weight, bool) + 'static,
) {
    let so_far = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let (tally, stopped) = (so_far.clone(), stop.clone());
    let work = cx.background_executor().spawn(async move {
        let mut shown = Instant::now();
        work(&mut |weight| {
            if shown.elapsed() >= WEIGH_EVERY {
                shown = Instant::now();
                *tally.lock() = Some(*weight);
            }
            !stopped.load(Ordering::Relaxed)
        })
    });
    let show = Rc::new(show);
    let (tick, done) = (show.clone(), stop.clone());
    cx.spawn(async move |this, cx| {
        while !done.load(Ordering::Relaxed) {
            cx.background_executor().timer(WEIGH_EVERY).await;
            let weight = so_far.lock().take();
            let open = this.update(cx, |view, cx| {
                if let Some(weight) = weight {
                    tick(view, weight, false);
                    cx.notify();
                }
            });
            if open.is_err() {
                done.store(true, Ordering::Relaxed);
            }
        }
    })
    .detach();
    cx.spawn(async move |this, cx| {
        let weight = work.await;
        stop.store(true, Ordering::Relaxed);
        this.update(cx, |view, cx| {
            show(view, weight, true);
            cx.notify();
        })
    })
    .detach();
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

fn icon_view(row: &Row, scale: f32) -> AnyElement {
    let path = row.path.clone();
    let centred = div().size_full().flex().items_center().justify_center();
    if !Path::new(path.as_ref()).is_absolute() && icon_of(&path).is_none() {
        return centred
            .child(row_art(row, PREVIEW, scale, true))
            .into_any_element();
    }
    let paths = vec![PathBuf::from(path.as_ref())];
    centred
        .child(
            div()
                .size(px(PREVIEW))
                .child(icon_well(paths, true, Some(move || icon_of(&path)))),
        )
        .into_any_element()
}

fn icon_file(path: &'static str) -> impl IntoElement {
    hosted(
        key_of(("ng.finder.info.icon_file", path)),
        move |mtm, _| {
            let view = NSImageView::new(mtm);
            let image =
                NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path));
            view.setImage(image.as_deref());
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view
        },
        |_, _| {},
    )
}

/// With no `image` the well paints nothing but keeps its menu, keys, drops and focus ring.
fn icon_well(
    paths: Vec<PathBuf>,
    same: bool,
    image: Option<impl Fn() -> Option<Retained<NSImage>> + 'static>,
) -> impl IntoElement {
    let key = key_of(("ng.finder.info.well", &paths));
    hosted(
        key,
        move |mtm, target| {
            let well = IconWell::new(mtm, paths.clone());
            wire(&well, target, key);
            well
        },
        move |well, host| {
            well.sync(same, image.as_ref());
            on_action(
                host,
                key,
                key,
                |well: &IconWell| well.take_event(),
                |event, _, cx| {
                    if let Some(event) = event {
                        icon_well::handle(event, cx);
                    }
                },
            );
        },
    )
}

fn flip(
    this: &WeakEntity<Info>,
    act: impl Fn(&mut Info, bool, &mut Context<Info>) + 'static,
) -> Flip {
    let this = this.clone();
    Rc::new(move |on, cx| {
        this.update(cx, |info, cx| act(info, on, cx)).ok();
    })
}

fn fits(form: &Form, label: &str, value: &str) -> bool {
    label == WHERE || text_width(value) <= form.room(label)
}

fn values_height(form: &Form, rows: &[(&str, SharedString)]) -> f32 {
    rows.iter()
        .map(|(label, value)| {
            if fits(form, label, value) {
                ROW
            } else {
                ROW + 8. + lines_height(value, form.inner)
            }
        })
        .sum()
}

fn values(form: &Form, rows: &[(&str, SharedString)]) -> Vec<AnyElement> {
    rows.iter()
        .map(|(label, value)| {
            if *label == WHERE {
                let room = form.room(label);
                return labelled(label, SystemColor::Label, where_control(value, room));
            }
            let shown = |text: &str| {
                native_label("value", text)
                    .color(SystemColor::SecondaryLabel)
                    .align(LabelAlign::Right)
            };
            if fits(form, label, value) {
                return labelled(label, SystemColor::Label, shown(value));
            }
            let tall = lines_height(value, form.inner);
            div()
                .h(px(ROW + 8. + tall))
                .p(px(INSET))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(native_label("label", label))
                .child(div().h(px(tall)).child(shown(value).wraps(true)))
                .into_any_element()
        })
        .collect()
}

fn name_height(form: &Form, name: &str) -> f32 {
    text_height(name, NAME, FontWeight::SEMIBOLD, form.wide)
}

fn card(id: &'static str, rows: Vec<AnyElement>) -> (f32, Part) {
    (STACK, Part::Card(id, rows))
}

fn header_height(form: &Form, name: &str, summary: bool, tagged: bool) -> f32 {
    let summary = if summary { 2. + LINE } else { 0. };
    let tags = if tagged { GAP + ROW } else { 0. };
    TOP + PREVIEW + INSET + name_height(form, name) + summary + tags
}

fn header(
    form: &Form,
    preview: impl IntoElement,
    name: &str,
    summary: &str,
    tags: Option<AnyElement>,
) -> AnyElement {
    div()
        .pt(px(TOP))
        .flex()
        .flex_col()
        .child(div().mx(px(MARGIN)).h(px(PREVIEW)).child(preview))
        .child(
            div()
                .mx(px(MARGIN))
                .mt(px(INSET))
                .h(px(name_height(form, name)))
                .child(
                    native_label("ng.finder.info.name", name)
                        .font_size(px(NAME))
                        .weight(FontWeight::SEMIBOLD)
                        .align(LabelAlign::Center)
                        .wraps(true),
                ),
        )
        .when(!summary.is_empty(), |head| {
            head.child(
                div().mx(px(MARGIN)).mt(px(2.)).child(
                    native_label("ng.finder.info.summary", summary)
                        .color(SystemColor::SecondaryLabel)
                        .align(LabelAlign::Center),
                ),
            )
        })
        .when_some(tags, |head, tags| {
            head.child(
                div()
                    .mt(px(GAP))
                    .child(form.card("ng.finder.info.tags", vec![tags])),
            )
        })
        .into_any_element()
}

fn bound(window: &Window, tall: f32) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    // AppKit keeps a window's frame on whole points.
    let tall = tall.ceil();
    let height = ns.contentRectForFrameRect(ns.frame()).size.height;
    let was = ns.contentMaxSize().height;
    let follows = was >= f64::from(f32::MAX) || (height - was).abs() < 0.5;
    let min = NSSize::new(NARROWEST.into(), SHORTEST.min(tall).into());
    let max = NSSize::new(WIDEST.into(), tall.into());
    if ns.contentMinSize() != min {
        ns.setContentMinSize(min);
    }
    if ns.contentMaxSize() != max {
        ns.setContentMaxSize(max);
    }
    let wanted = follows.then_some(f64::from(tall));
    resize_later(ns, move |ns| refit(ns, wanted));
}

fn refit(ns: &NSWindow, wanted: Option<f64>) -> Option<NSRect> {
    let content = ns.contentRectForFrameRect(ns.frame());
    let (min, max) = (ns.contentMinSize(), ns.contentMaxSize());
    let top = content.origin.y + content.size.height;
    let room = ns
        .screen()
        .map_or(f64::MAX, |screen| top - screen.visibleFrame().origin.y)
        .max(content.size.height);
    let height = wanted.map_or(content.size.height, |wanted| wanted.min(room));
    let fitted = NSSize::new(
        content.size.width.min(max.width).max(min.width),
        height.min(max.height).max(min.height),
    );
    if (fitted.width - content.size.width).abs() < 0.5
        && (fitted.height - content.size.height).abs() < 0.5
    {
        return None;
    }
    let rect = NSRect::new(NSPoint::new(content.origin.x, top - fitted.height), fitted);
    Some(ns.frameRectForContentRect(rect))
}

// AppKit holds only a user's drag to contentMaxSize; a setFrame from code goes through unclamped.
fn keep_fitted(ns: &NSWindow) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let window = Weak::new(ns);
    let block = StackBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(window) = window.load() {
            if let Some(frame) = refit(&window, None) {
                window.setFrame_display(frame, true);
            }
        }
    })
    .copy();
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidResizeNotification),
            Some(ns),
            None,
            &block,
        )
    }
}

type Crumbs = Rc<[(&'static str, &'static str)]>;

fn crumbs_at(path: &str) -> Crumbs {
    thread_local! {
        static SEEN: RefCell<HashMap<&'static str, Crumbs>> = RefCell::default();
    }
    let id = intern_path(Path::new(path));
    SEEN.with_borrow_mut(|seen| {
        if seen.len() >= KEPT {
            seen.clear();
        }
        seen.entry(id)
            .or_insert_with(|| folder_of(id).crumbs.into())
            .clone()
    })
}

fn crumb_icon(id: &str) -> Retained<NSImage> {
    if crate::remote::is_remote(id) {
        let name = match crate::remote::artwork(id) {
            FileType::Named(name) => name,
            _ => "NSFolder",
        };
        return NSImage::imageNamed(&NSString::from_str(name)).unwrap_or_default();
    }
    if icloud_drive() == Some(id) {
        if let FileType::Named(name) = crate::chrome::path_bar::icloud_art() {
            if let Some(art) = NSImage::imageNamed(&NSString::from_str(name)) {
                return art;
            }
        }
    }
    NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(id))
}

fn show_crumbs(control: &NSPathControl, crumbs: &[(&'static str, &'static str)], shown: usize) {
    let hidden = crumbs.len().saturating_sub(shown);
    let first = match hidden {
        0 => crumbs.first().map_or("", |&(name, _)| name),
        _ => "\u{2026}",
    };
    let items = control.pathItems();
    let same = items.count() == crumbs.len() - hidden + usize::from(hidden > 0)
        && items
            .firstObject()
            .is_some_and(|item| item.title().to_string() == first);
    if same {
        return;
    }
    let more = (hidden > 0).then(|| {
        let item = NSPathControlItem::new();
        item.setTitle(ns_string!("\u{2026}"));
        item
    });
    let items: Vec<Retained<NSPathControlItem>> = more
        .into_iter()
        .chain(crumbs[hidden..].iter().map(|&(name, id)| {
            let item = NSPathControlItem::new();
            item.setTitle(&NSString::from_str(name));
            item.setImage(Some(&crumb_icon(id)));
            item
        }))
        .collect();
    control.setPathItems(&NSArray::from_retained_slice(&items));
}

fn path_control(mtm: MainThreadMarker) -> Retained<NSPathControl> {
    let control = NSPathControl::new(mtm);
    control.setFont(Some(&NSFont::systemFontOfSize(BODY.into())));
    control
}

fn crumbs_fit(path: &str, crumbs: &[(&'static str, &'static str)], room: f32) -> (usize, f32, f32) {
    thread_local! {
        static SEEN: RefCell<HashMap<u64, (usize, f32, f32)>> = RefCell::default();
    }
    let room = room.floor();
    remembered(&SEEN, key_of((path, room as i32)), || {
        let control = path_control(MainThreadMarker::new().expect("laid out on the main thread"));
        let mut shown = crumbs.len();
        loop {
            show_crumbs(&control, crumbs, shown);
            let fitting = control.fittingSize();
            if shown <= 1 || fitting.width as f32 <= room {
                return (
                    shown,
                    fitting.width.ceil() as f32,
                    fitting.height.ceil() as f32,
                );
            }
            shown -= 1;
        }
    })
}

fn where_control(path: &SharedString, room: f32) -> impl IntoElement {
    let key = key_of(("ng.finder.info.where", path));
    let room = room + PATH_TRAIL;
    let crumbs = crumbs_at(path);
    let (shown, wide, tall) = crumbs_fit(path, &crumbs, room);
    let hidden = crumbs.len().saturating_sub(shown);
    let ids: Vec<&'static str> = crumbs[hidden..].iter().map(|&(_, id)| id).collect();
    let tip = NSString::from_str(path);
    div()
        .w(px(wide.min(room)))
        .h(px(tall))
        .mr(px(-PATH_TRAIL))
        .child(hosted(
            key,
            move |mtm, target| {
                let control = path_control(mtm);
                control.setToolTip(Some(&tip));
                wire(&control, target, key);
                control
            },
            move |control, host| {
                show_crumbs(control, &crumbs, shown);
                let ids = ids.clone();
                on_action(
                    host,
                    key,
                    key,
                    move |control: &NSPathControl| {
                        let clicked = control.clickedPathItem()?;
                        let at = control.pathItems().iter().position(|item| {
                            Retained::as_ptr(&item) == Retained::as_ptr(&clicked)
                        })?;
                        ids.get(at.checked_sub(usize::from(hidden > 0))?).copied()
                    },
                    |id, _, cx| {
                        if let Some(id) = id {
                            show_folder(id, cx);
                        }
                    },
                );
            },
        ))
}

fn show_folder(dir: &'static str, cx: &mut App) {
    let shown = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<FinderDemo>())
        .find(|window| window.read(cx).is_ok_and(|demo| demo.here() == dir));
    let window = shown.unwrap_or_else(|| crate::open_finder_window(dir, cx));
    window
        .update(cx, |_, window, _| window.activate_window())
        .ok();
}

struct Opener {
    apps: Vec<PathBuf>,
    chosen: Option<PathBuf>,
    default: Option<PathBuf>,
}

impl Opener {
    fn of(path: &Path) -> Self {
        let default = crate::openwith::type_default(path);
        let mut apps: Vec<PathBuf> = crate::openwith::apps_for(path)
            .into_iter()
            .map(|app| app.path)
            .collect();
        if let Some(at) = default
            .as_ref()
            .and_then(|default| apps.iter().position(|app| app == default))
        {
            apps[..=at].rotate_right(1);
        }
        Opener {
            chosen: crate::openwith::opener(path),
            apps,
            default,
        }
    }

    fn rows(&self) -> Vec<Pick> {
        let mut rows = Vec::new();
        for (at, app) in self.apps.iter().enumerate() {
            if at == 1 && self.default.is_some() {
                rows.push(Pick::Separator);
            }
            rows.push(Pick::App(app.clone()));
        }
        if !rows.is_empty() {
            rows.push(Pick::Separator);
        }
        rows.extend([Pick::Store, Pick::Other]);
        rows
    }
}

enum Pick {
    App(PathBuf),
    Separator,
    Store,
    Other,
}

impl Pick {
    fn title(&self, default: Option<&Path>) -> String {
        match self {
            Pick::App(app) => {
                let name = finder_fs::name::of_path(app);
                if default == Some(app.as_path()) {
                    format!("{name} (default)")
                } else {
                    name
                }
            }
            Pick::Separator => String::new(),
            Pick::Store => "App Store\u{2026}".to_owned(),
            Pick::Other => "Other\u{2026}".to_owned(),
        }
    }

    fn entry(&self, default: Option<&Path>) -> MenuEntry {
        match self {
            Pick::Separator => MenuEntry::separator(),
            Pick::App(app) => MenuEntry::item(self.title(default))
                .file_icon(FileType::Path(&app.to_string_lossy())),
            _ => MenuEntry::item(self.title(default)),
        }
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
    demo: WeakEntity<FinderDemo>,
    tags: Option<Rc<[Tag]>>,
    weight: Option<Weight>,
    so_far: Option<Weight>,
    weighing: bool,
    metadata: Option<Meta>,
    fetching: bool,
    owned: bool,
    locked: bool,
    stationery: bool,
    renamable: bool,
    hidden_extension: bool,
    following: Option<Task<()>>,
    titled: Option<SharedString>,
    opener: Option<Opener>,
    panel: bool,
    width: f32,
    preview: Shown,
    access: Option<Access>,
    reading: bool,
    first_mode: Option<u32>,
    custom_icon: bool,
    icon_seen: u64,
    folds: [Fold; 6],
}

impl Info {
    pub(crate) fn new(line: Line, where_: SharedString, demo: WeakEntity<FinderDemo>) -> Self {
        let path = Path::new(line.row.path.as_ref());
        let subject = Subject::of(&line.row);
        let meta = std::fs::symlink_metadata(path).ok();
        let real = meta.is_some();
        Info {
            weight: match (subject, &meta) {
                (Subject::File, Some(meta)) if !meta.is_dir() => Some(Weight::file(meta)),
                _ => None,
            },
            tags: (real && subject != Subject::Place).then(|| held_tags(path).into()),
            owned: meta.is_some_and(|meta| {
                meta.uid() == unsafe { getuid() } || finder_fs::volumes::ignores_ownership(path)
            }),
            locked: real && finder_fs::ops::locked(path),
            stationery: subject == Subject::File && finder_fs::flags::of(path) & IS_STATIONERY != 0,
            renamable: real
                && matches!(subject, Subject::File | Subject::Folder | Subject::App)
                && path.parent().is_some_and(writable),
            hidden_extension: real && resource(path, unsafe { NSURLHasHiddenExtensionKey }),
            first_mode: real.then(|| permissions::mode(path).ok()).flatten(),
            custom_icon: real && native_gpui::tokens::has_custom_icon(path),
            icon_seen: icon_well::generation(),
            line,
            where_,
            subject,
            demo,
            so_far: None,
            weighing: false,
            metadata: None,
            fetching: false,
            following: None,
            titled: None,
            opener: None,
            panel: false,
            width: info_width(),
            preview: Shown::Icon,
            access: None,
            reading: false,
            folds: panes_open().map(Fold::new),
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
        weigh_in_background(
            cx,
            move |report| weigh_folder(&path, counted, report),
            |info, weight, done| {
                if done {
                    info.weight = Some(weight);
                } else {
                    info.so_far = Some(weight);
                }
            },
        );
    }

    fn fetch_metadata(&mut self, cx: &mut Context<Self>) {
        if self.subject == Subject::Place || self.fetching {
            return;
        }
        self.fetching = true;
        let path = self.path().to_path_buf();
        in_background(
            cx,
            move || finder_fs::meta::meta(&path),
            |info, metadata| info.metadata = Some(metadata),
        );
    }

    fn ensure_opener(&mut self) {
        if self.subject == Subject::File && self.opener.is_none() {
            self.opener = Some(Opener::of(self.path()));
        }
    }

    fn ensure_access(&mut self, cx: &mut Context<Self>) {
        if self.subject != Subject::Place && self.access.is_none() && !self.reading {
            self.reading = true;
            self.read_access(cx);
        }
    }

    fn read_access(&mut self, cx: &mut Context<Self>) {
        let path = self.path().to_path_buf();
        let subject = self.subject;
        in_background(
            cx,
            move || Access::of(&path, subject),
            |info, access| info.access = Some(access),
        );
    }

    fn set_check(&mut self, check: Check, on: bool, cx: &mut Context<Self>) {
        match check {
            Check::Stationery => {
                if finder_fs::flags::set(self.path(), IS_STATIONERY, on).is_ok() {
                    self.stationery = on;
                }
            }
            Check::Locked => {
                if set_resource(self.path(), unsafe { NSURLIsUserImmutableKey }, on) {
                    self.locked = on;
                }
            }
        }
        cx.notify();
    }

    fn title(&mut self, window: &mut Window) {
        let display = &self.line.row.display;
        if self.titled.as_ref() == Some(display) {
            return;
        }
        window.set_window_title(&format!("{display} Info"));
        if let Some(ns) = ns_window(window).filter(|_| !self.panel) {
            let shown = crate::openwith::url(self.path()).filter(|_| self.path().is_absolute());
            ns.setRepresentedURL(shown.as_deref());
        }
        self.titled = Some(display.clone());
    }

    fn follow_icon(&mut self, window: &Window) {
        let generation = icon_well::generation();
        if self.icon_seen == generation {
            return;
        }
        self.icon_seen = generation;
        let path = Path::new(self.line.row.path.as_ref());
        self.custom_icon = path.is_absolute() && native_gpui::tokens::has_custom_icon(path);
        let proxy = ns_window(window)
            .filter(|_| !self.panel && path.is_absolute())
            .and_then(|ns| ns.standardWindowButton(NSWindowButton::DocumentIconButton));
        if let Some(proxy) = proxy {
            let icon = NSWorkspace::sharedWorkspace()
                .iconForFile(&NSString::from_str(&self.line.row.path));
            proxy.setImage(Some(&icon));
        }
    }

    fn rename(&mut self, name: &str, cx: &mut Context<Self>) {
        let from = self.path().to_path_buf();
        let wanted = name.trim().replace('/', ":");
        if wanted.is_empty() || from.file_name().is_some_and(|old| *old == *wanted) {
            return;
        }
        let Some(demo) = self.demo.upgrade() else {
            return;
        };
        demo.update(cx, |demo, cx| demo.rename(&self.line.row.path, name, cx));
        let to = from.with_file_name(&wanted);
        let inode = std::fs::symlink_metadata(&from).ok().map(|meta| meta.ino());
        self.following = Some(cx.spawn(async move |this, cx| {
            for _ in 0..FOLLOW_TRIES {
                cx.background_executor().timer(FOLLOW_EVERY).await;
                if std::fs::symlink_metadata(&to).ok().map(|meta| meta.ino()) == inode {
                    if let Some(line) = crate::data::path_line(intern_path(&to)) {
                        this.update(cx, |info, cx| {
                            info.line = line;
                            cx.notify();
                        })
                        .ok();
                    }
                    return;
                }
            }
        }));
    }

    fn hide_extension(&mut self, on: bool, cx: &mut Context<Self>) {
        if set_resource(self.path(), unsafe { NSURLHasHiddenExtensionKey }, on) {
            self.hidden_extension = on;
            if let Some(line) = crate::data::path_line(intern_path(self.path())) {
                self.line = line;
            }
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

    fn summary(&self) -> String {
        let row = &self.line.row;
        let mut parts = vec![row.kind.to_string()];
        match self.subject {
            Subject::Place => {}
            Subject::Volume => {
                parts.extend(volume_at(&row.path).map(|volume| byte_string(volume.capacity)))
            }
            _ => {
                if let Some(weight) = self.weight {
                    parts.push(byte_string(weight.bytes));
                    parts.extend(weight.items.map(items_text));
                }
            }
        }
        parts.retain(|part| !part.is_empty());
        parts.join(" \u{b7} ")
    }

    fn general_rows(&self, form: &Form) -> Vec<(&'static str, SharedString)> {
        let row = &self.line.row;
        let date =
            |label: &str, seconds: f64| SharedString::from(long_date(seconds, form.room(label)));
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
                    ("Created", date("Created", row.created_at)),
                    ("Modified", date("Modified", row.modified_at)),
                ];
                if let Some(version) = system_version(&row.path) {
                    rows.push(("Version", version.into()));
                }
                rows.extend([
                    ("Format", volume.format.into()),
                    ("Capacity", byte_string(volume.capacity).into()),
                    ("Available", byte_string(volume.available).into()),
                    ("Used", size_text(used).into()),
                ]);
                rows
            }
            _ => {
                let kind = match self.subject {
                    Subject::App => app_kind(self.path(), &row.kind).into(),
                    _ => row.kind.clone(),
                };
                let mut rows = vec![("Kind", kind)];
                rows.extend(weight_rows(self.weight, self.so_far));
                if !self.where_.is_empty() {
                    rows.push((WHERE, self.where_.clone()));
                }
                rows.extend([
                    ("Created", date("Created", row.created_at)),
                    ("Modified", date("Modified", row.modified_at)),
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

    fn more_rows(&self, form: &Form) -> Vec<(&'static str, SharedString)> {
        let Some(metadata) = &self.metadata else {
            return Vec::new();
        };
        let value = |name: &str| {
            metadata
                .values
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
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
        if let Some(at) = metadata.last_used {
            rows.push((
                "Last opened",
                long_date(at, form.room("Last opened")).into(),
            ));
        }
        rows
    }

    fn default_height(&self, width: f32) -> f32 {
        let form = Form::new(width, 1.);
        let head = header_height(
            &form,
            &self.line.row.display,
            !self.summary().is_empty(),
            self.tags.is_some(),
        );
        let toggles = match self.checks().len() {
            0 => 0.,
            checks => STACK + checks as f32 * ROW,
        };
        let general = SECTION + TITLE + values_height(&form, &self.general_rows(&form)) + toggles;
        TITLEBAR + head + general + BOTTOM
    }

    fn shown(&self, scale: f32) -> Shown {
        let row = &self.line.row;
        if self.subject != Subject::File || self.custom_icon {
            return Shown::Icon;
        }
        match crate::thumbs::thumb(row, THUMB, scale, true, false) {
            Some(image) if matches!(row.face, Face::Image | Face::Movie) => Shown::Picture(image),
            Some(image) => Shown::Page(image),
            None => Shown::Icon,
        }
    }

    fn preview(&self, form: &Form, scale: f32) -> AnyElement {
        let (image, page) = match &self.preview {
            Shown::Icon => return icon_view(&self.line.row, scale),
            Shown::Picture(image) => (image, false),
            Shown::Page(image) => (image, true),
        };
        let tall = (form.wide * aspect(image)).min(PREVIEW);
        let wide = tall / aspect(image);
        let edge = if page { 1. } else { 0. };
        let shown = img(image.clone())
            .w(px(wide - 2. * edge))
            .h(px(tall - 2. * edge))
            .rounded(px(4. - edge));
        div()
            .w_full()
            .h(px(PREVIEW))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(wide))
                    .h(px(tall))
                    .relative()
                    .rounded(px(4.))
                    .when(page, |page| {
                        page.border_1()
                            .border_color(system_color(SystemColor::Separator))
                            .bg(system_color(SystemColor::TextBackground))
                    })
                    .child(shown)
                    .child(div().absolute().inset_0().child(icon_well(
                        vec![self.path().to_path_buf()],
                        true,
                        None::<fn() -> _>,
                    ))),
            )
            .into_any_element()
    }

    fn head(&self, form: &Form, scale: f32) -> AnyElement {
        let row = &self.line.row;
        let tags = self.tags.as_ref().map(|tags| {
            div()
                .h(px(ROW))
                .pl(px(INSET - 2.))
                .pr(px(INSET))
                .pt(px(9.))
                .pb(px(6.))
                .child(tags_field(row.path.clone(), tags.clone()))
                .into_any_element()
        });
        header(
            form,
            self.preview(form, scale),
            &row.display,
            &self.summary(),
            tags,
        )
    }

    fn pane(
        &self,
        form: &Form,
        this: &WeakEntity<Self>,
        pane: Pane,
        parts: Vec<(f32, Part)>,
    ) -> AnyElement {
        let this = this.clone();
        let toggle = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |info, cx| info.fold(pane, cx)).ok();
        };
        let (id, title) = pane.section();
        fold::section(form, id, title, &self.folds[pane as usize], toggle, parts).into_any_element()
    }

    fn fold(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let fold = &mut self.folds[pane as usize];
        fold.toggle();
        remember(pane, fold.is_open());
        cx.notify();
    }

    fn general(&self, form: &Form, this: &WeakEntity<Self>) -> AnyElement {
        let toggles: Vec<AnyElement> = self
            .checks()
            .into_iter()
            .map(|(title, on, check)| {
                let change = check
                    .map(|check| flip(this, move |info, on, cx| info.set_check(check, on, cx)));
                switch_row(title, on, change)
            })
            .collect();
        let mut parts = vec![card("values", values(form, &self.general_rows(form)))];
        if !toggles.is_empty() {
            parts.push(card("toggles", toggles));
        }
        self.pane(form, this, Pane::General, parts)
    }

    fn more_info(&self, form: &Form, this: &WeakEntity<Self>) -> Option<AnyElement> {
        let rows = self.more_rows(form);
        (!rows.is_empty()).then(|| {
            let parts = vec![card("values", values(form, &rows))];
            self.pane(form, this, Pane::MetaData, parts)
        })
    }

    fn name_section(&self, form: &Form, this: &WeakEntity<Self>) -> AnyElement {
        let renamable = self.renamable && !self.locked;
        let field = div().w_full().h(px(LINE)).child(name_field(
            self.line.row.path.clone(),
            renamable,
            this.clone(),
        ));
        let hide = (renamable && self.has_extension()).then(|| flip(this, Info::hide_extension));
        let rows = vec![
            labelled("Name", SystemColor::Label, field),
            switch_row("Hide extension", self.hidden_extension, hide),
        ];
        self.pane(form, this, Pane::Name, vec![card("card", rows)])
    }

    fn comments(&self, form: &Form, this: &WeakEntity<Self>) -> AnyElement {
        let editable = self.renamable && !self.locked && writable(self.path());
        let text = div().p(px(INSET)).child(
            div()
                .h(px(COMMENT))
                .child(comment_box(self.line.row.path.clone(), editable)),
        );
        let parts = vec![card("card", vec![text.into_any_element()])];
        self.pane(form, this, Pane::Comments, parts)
    }

    fn open_with(&self, form: &Form, this: &WeakEntity<Self>) -> Option<AnyElement> {
        let opener = self.opener.as_ref()?;
        let rows = opener.rows();
        let chosen = rows
            .iter()
            .position(|row| matches!(row, Pick::App(app) if opener.chosen.as_ref() == Some(app)))
            .unwrap_or(0);
        let default = opener.default.as_deref();
        let picked = rows.get(chosen);
        let shown = picked.map(|row| row.title(default)).unwrap_or_default();
        let pick = this.clone();
        let popup = native_popup("popup", chosen)
            .entries(rows.iter().map(|row| row.entry(default)))
            .bordered(false)
            .width(px(hugging(&shown, matches!(picked, Some(Pick::App(_))))))
            .on_change(move |&row, _, cx| {
                pick.update(cx, |info, cx| info.pick(row, cx)).ok();
            });
        let change = this.clone();
        let button = native_button("change", "Change All\u{2026}")
            .enabled(opener.chosen.is_some() && opener.chosen != opener.default)
            .on_click(move |_, window, cx| {
                let ns = ns_window(window);
                change.update(cx, |info, cx| info.change_all(ns, cx)).ok();
            });
        let note = lines_height(NOTE, form.inner - change_all_width() - GAP);
        let all = div()
            .h(px(note.max(24.) + 2. * INSET))
            .px(px(INSET))
            .flex()
            .items_center()
            .gap(px(GAP))
            .child(
                div().flex_1().h(px(note)).child(
                    native_label("note", NOTE)
                        .color(SystemColor::SecondaryLabel)
                        .wraps(true),
                ),
            )
            .child(button);
        let rows = vec![
            labelled("Application", SystemColor::Label, popup),
            all.into_any_element(),
        ];
        Some(self.pane(form, this, Pane::OpenWith, vec![card("card", rows)]))
    }

    fn privileges(&self, form: &Form, this: &WeakEntity<Self>) -> Option<AnyElement> {
        let access = self.access.as_ref()?;
        let path = &self.line.row.path;
        let folder = matches!(self.subject, Subject::Folder | Subject::Volume);
        let grants = match &access.grants {
            Ok(grants) => (grants.iter().enumerate())
                .map(|(row, grant)| grant_row(row, grant, folder, self.owned, this.clone()))
                .collect(),
            Err(why) => vec![
                div()
                    .h(px(ROW))
                    .px(px(INSET))
                    .flex()
                    .items_center()
                    .child(native_label("why", why).color(SystemColor::SecondaryLabel))
                    .into_any_element(),
            ],
        };
        let changed = access.mode.is_some() && access.mode != self.first_mode;
        let tools = tools_size();
        let yours = div().px(px(MARGIN + INSET)).child(
            native_label("yours", yours_text(access.yours)).color(SystemColor::SecondaryLabel),
        );
        let mut parts = vec![
            (4., Part::Plain(yours.into_any_element())),
            card("grants", grants),
        ];
        if let Some(ignores) = access.ownership {
            let row = switch_row("Ignore ownership on this volume", ignores, None);
            parts.push(card("ownership", vec![row]));
        }
        let controls = div()
            .mx(px(MARGIN))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .w(px(tools.width as f32))
                    .h(px(tools.height as f32))
                    .child(grant_tools(path, self.owned, folder, changed, this.clone())),
            )
            .child(div().size(px(18.)).child(icon_file(LOCK_ICON)));
        parts.push((STACK, Part::Plain(controls.into_any_element())));
        Some(self.pane(form, this, Pane::Privileges, parts))
    }

    fn grant(&mut self, row: usize, privilege: Privilege, cx: &mut Context<Self>) {
        let class = self
            .access
            .as_ref()
            .and_then(|access| access.grants.as_ref().ok()?.get(row)?.class);
        if let Some(class) = class {
            permissions::set(self.path(), class, privilege).ok();
        }
        self.read_access(cx);
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        if let Some(mode) = self.first_mode {
            permissions::set_mode(self.path(), mode).ok();
        }
        self.read_access(cx);
    }

    fn confirm_apply(&self, window: Option<Retained<NSWindow>>, cx: &mut Context<Self>) {
        let Some(window) = window else {
            return;
        };
        let path = self.path().to_path_buf();
        let background = cx.background_executor().clone();
        confirm(
            &window,
            ns_string!(
                "Are you sure you want to apply the selected owner, group, and permissions to all the enclosed items?"
            ),
            ns_string!("You can\u{2019}t undo this."),
            ns_string!("OK"),
            move || {
                let path = path.clone();
                background
                    .spawn(async move { permissions::apply_to_enclosed(&path) })
                    .detach();
            },
        );
    }

    fn pick(&mut self, row: usize, cx: &mut Context<Self>) {
        let path = self.path().to_path_buf();
        let Some(opener) = &mut self.opener else {
            return;
        };
        match opener.rows().into_iter().nth(row) {
            Some(Pick::App(app)) => {
                crate::openwith::set_default(&[path], &app);
                opener.chosen = Some(app);
            }
            Some(Pick::Store) => crate::openwith::app_store(&[path]),
            Some(Pick::Other) => {
                choose_file(cx, Some("/Applications".into()), move |info, app, cx| {
                    crate::openwith::set_default(std::slice::from_ref(&path), &app);
                    let mut opener = Opener::of(&path);
                    if !opener.apps.contains(&app) {
                        opener.apps.push(app.clone());
                    }
                    opener.chosen = Some(app);
                    info.opener = Some(opener);
                    cx.notify();
                })
            }
            Some(Pick::Separator) | None => {}
        }
        cx.notify();
    }

    fn change_all(&self, window: Option<Retained<NSWindow>>, cx: &mut Context<Self>) {
        let app = self
            .opener
            .as_ref()
            .and_then(|opener| opener.chosen.clone());
        let (Some(app), Some(window)) = (app, window) else {
            return;
        };
        let message = format!(
            "Are you sure you want to change all similar documents to open with the application \u{201c}{}\u{201d}?",
            finder_fs::name::of_path(&app)
        );
        let kind = match self.path().extension() {
            Some(extension) => format!("extension \u{201c}{}\u{201d}", extension.to_string_lossy()),
            None => format!("type \u{201c}{}\u{201d}", self.line.row.kind),
        };
        let detail = format!("This change will apply to all documents with {kind}.");
        let this = cx.weak_entity();
        let async_cx = cx.to_async();
        confirm(
            &window,
            &NSString::from_str(&message),
            &NSString::from_str(&detail),
            ns_string!("Continue"),
            move || {
                let mut cx = async_cx.clone();
                this.update(&mut cx, |info, cx| info.make_default(&app, cx))
                    .ok();
            },
        );
    }

    fn make_default(&mut self, app: &Path, cx: &mut Context<Self>) {
        crate::openwith::set_type_default(self.path(), app);
        if let Some(opener) = &mut self.opener {
            opener.apps.retain(|known| known != app);
            opener.apps.insert(0, app.to_path_buf());
            opener.default = Some(app.to_path_buf());
        }
        cx.notify();
    }
}

impl Render for Info {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        let scale = window.scale_factor();
        if !self.panel && (width - self.width).abs() >= 0.5 {
            prefs::set_setting_number(WIDTH_KEY, width.into());
        }
        self.width = width;
        self.weigh(cx);
        self.fetch_metadata(cx);
        self.ensure_opener();
        self.follow_icon(window);
        self.preview = self.shown(scale);
        self.ensure_access(cx);
        cx.defer(|_| crate::thumbs::collect_thumbnails());
        self.title(window);
        let this = cx.weak_entity();
        let form = Form::new(width, 1. / scale);
        let mut content = vec![self.head(&form, scale), self.general(&form, &this)];
        if self.subject != Subject::Place {
            content.extend(self.more_info(&form, &this));
            content.push(self.name_section(&form, &this));
            content.push(self.comments(&form, &this));
            content.extend(self.open_with(&form, &this));
            content.extend(self.privileges(&form, &this));
        }
        if self.folds.iter().any(Fold::moving) {
            window.request_animation_frame();
        }
        page(window, content, BOTTOM, bound)
    }
}

fn confirm(
    window: &NSWindow,
    message: &NSString,
    detail: &NSString,
    ok: &NSString,
    on_ok: impl Fn() + 'static,
) {
    let alert = NSAlert::new(window.mtm());
    alert.setMessageText(message);
    alert.setInformativeText(detail);
    alert.addButtonWithTitle(ok);
    alert.addButtonWithTitle(ns_string!("Cancel"));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response == NSAlertFirstButtonReturn {
            on_ok();
        }
    });
    alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
}

fn bare_field(field: &NSTextField) {
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(false);
    if let Some(cell) = field.cell() {
        cell.setSendsActionOnEndEditing(true);
    }
}

fn tags_field(path: SharedString, tags: Rc<[Tag]>) -> impl IntoElement {
    let key = key_of(("ng.finder.info.tags", &path));
    hosted(
        key,
        move |mtm, target| {
            let field = tag_field(mtm, NSRect::ZERO, &tags);
            bare_field(&field);
            field.setFocusRingType(NSFocusRingType::None);
            field.setPlaceholderString(Some(ns_string!("Add Tags…")));
            wire(&field, target, key);
            field
        },
        move |_, host| {
            let path = path.clone();
            on_action(host, key, key, tags_in, move |tags, _, cx| {
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
            });
        },
    )
}

fn name_field(path: SharedString, enabled: bool, this: WeakEntity<Info>) -> impl IntoElement {
    let key = key_of(("ng.finder.info.name.field", &path));
    hosted(
        key,
        move |mtm, target| {
            let field = NSTextField::new(mtm);
            bare_field(&field);
            field.setAlignment(NSTextAlignment::Right);
            field.setFont(Some(&NSFont::systemFontOfSize(BODY.into())));
            if let Some(name) = Path::new(path.as_ref()).file_name() {
                let name = name.to_string_lossy();
                field.setStringValue(&NSString::from_str(&finder_fs::name::display(&name)));
            }
            if let Some(cell) = field.cell() {
                cell.setScrollable(true);
            }
            wire(&field, target, key);
            field
        },
        move |field, host| {
            if field.isEnabled() != enabled {
                field.setEnabled(enabled);
            }
            let this = this.clone();
            on_action(
                host,
                key,
                key,
                |field: &NSTextField| field.stringValue().to_string(),
                move |name, _, cx| {
                    this.update(cx, |info, cx| info.rename(&name, cx)).ok();
                },
            );
        },
    )
}

struct Access {
    grants: Result<Vec<Grant>, String>,
    yours: Privilege,
    mode: Option<u32>,
    ownership: Option<bool>,
}

impl Access {
    fn of(path: &Path, subject: Subject) -> Self {
        Access {
            grants: permissions::grants(path).map_err(|error| error.to_string()),
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

fn grant_row(
    row: usize,
    grant: &Grant,
    folder: bool,
    editable: bool,
    this: WeakEntity<Info>,
) -> AnyElement {
    let choices: Vec<Privilege> = PRIVILEGES
        .into_iter()
        .filter(|&choice| {
            choice == grant.privilege
                || match choice {
                    Privilege::WriteOnly => folder,
                    Privilege::Custom => false,
                    _ => true,
                }
        })
        .collect();
    let chosen = choices
        .iter()
        .position(|&choice| choice == grant.privilege)
        .unwrap_or(0);
    let symbol = if grant.group {
        "person.2.fill"
    } else {
        "person.crop.circle"
    };
    let name = if grant.me {
        format!("{} (Me)", grant.name)
    } else {
        grant.name.clone()
    };
    let popup = native_popup("privilege", chosen)
        .items(choices.iter().map(|&choice| privilege_title(choice)))
        .bordered(false)
        .width(px(hugging(privilege_title(grant.privilege), false)))
        .enabled(editable && grant.class.is_some())
        .on_change(move |&at, _, cx| {
            if let Some(&privilege) = choices.get(at) {
                this.update(cx, |info, cx| info.grant(row, privilege, cx))
                    .ok();
            }
        });
    div()
        .h(px(ROW))
        .px(px(INSET))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .w(px(20.))
                .flex()
                .justify_center()
                .children(small_glyph(
                    symbol,
                    BODY,
                    FontWeight::NORMAL,
                    SystemColor::Label,
                )),
        )
        .child(div().flex_1().child(native_label("name", &name)))
        .child(popup)
        .into_any_element()
}

fn change_all_width() -> f32 {
    native_gpui::fitting_size("NSButton.info_change_all", |mtm| unsafe {
        NSButton::buttonWithTitle_target_action(ns_string!("Change All\u{2026}"), None, None, mtm)
            .fittingSize()
    })
    .width as f32
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

/// Add and Remove stay off: adding a user needs a directory-service picker.
fn tools_control(mtm: MainThreadMarker) -> Retained<NSSegmentedControl> {
    let control = NSSegmentedControl::new(mtm);
    control.setSegmentStyle(NSSegmentStyle::Rounded);
    control.setTrackingMode(NSSegmentSwitchTracking::Momentary);
    control.setControlSize(NSControlSize::Small);
    control.setSegmentCount(3);
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(11., unsafe {
        NSFontWeightRegular
    });
    for (at, (symbol, tip)) in (0..).zip([
        (ns_string!("plus"), ns_string!("Add")),
        (ns_string!("minus"), ns_string!("Remove")),
        (ns_string!("ellipsis.circle"), ns_string!("Action")),
    ]) {
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(symbol, Some(tip))
            .and_then(|image| image.imageWithSymbolConfiguration(&config));
        control.setImage_forSegment(image.as_deref(), at);
        control.setToolTip_forSegment(Some(tip), at);
        control.setEnabled_forSegment(false, at);
    }
    control.setMenu_forSegment(Some(&NSMenu::new(mtm)), 2);
    control.setShowsMenuIndicator_forSegment(true, 2);
    control
}

fn tools_size() -> NSSize {
    native_gpui::fitting_size("NSSegmentedControl.info_tools", |mtm| {
        tools_control(mtm).intrinsicContentSize()
    })
}

fn grant_tools(
    path: &str,
    editable: bool,
    folder: bool,
    changed: bool,
    this: WeakEntity<Info>,
) -> impl IntoElement {
    let key = key_of(("ng.finder.info.grants.tools", path, folder));
    let apply = key_of(("ng.finder.info.grants.apply", path));
    let revert = key_of(("ng.finder.info.grants.revert", path));
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
            if let Some(item) = control
                .menuForSegment(2)
                .and_then(|menu| menu.itemWithTag(revert as isize))
                .filter(|item| item.isEnabled() != changed)
            {
                item.setEnabled(changed);
            }
            let to_revert = this.clone();
            on_action(
                host,
                key,
                revert,
                |_: &NSObject| (),
                move |_, _, cx| {
                    to_revert.update(cx, |info, cx| info.revert(cx)).ok();
                },
            );
            let to_apply = this.clone();
            on_action(
                host,
                key,
                apply,
                |_: &NSObject| (),
                move |_, window, cx| {
                    let ns = ns_window(window);
                    to_apply
                        .update(cx, |info, cx| info.confirm_apply(ns, cx))
                        .ok();
                },
            );
        },
    )
}

struct CommentIvars {
    path: PathBuf,
    saved: RefCell<String>,
}

define_class!(
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
        let saved = finder_fs::comment::of(&path);
        let shown = NSString::from_str(&saved);
        let this = Self::alloc(mtm).set_ivars(CommentIvars {
            path,
            saved: RefCell::new(saved),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setString(&shown);
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
        key_of(("ng.finder.info.comment", &path)),
        move |mtm, _| {
            let scroll = NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(100., COMMENT.into())),
            );
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            let size = scroll.contentSize();
            let text = Comment::new(
                mtm,
                PathBuf::from(path.as_ref()),
                NSRect::new(NSPoint::ZERO, size),
            );
            text.setMinSize(NSSize::new(0., size.height));
            text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
            text.setVerticallyResizable(true);
            text.setHorizontallyResizable(false);
            text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            if let Some(container) = unsafe { text.textContainer() } {
                container.setWidthTracksTextView(true);
                container.setLineFragmentPadding(0.);
            }
            text.setDrawsBackground(false);
            text.setRichText(false);
            text.setAllowsUndo(true);
            text.setFont(Some(&NSFont::systemFontOfSize(BODY.into())));
            text.setTextColor(Some(&NSColor::labelColor()));
            scroll.setDocumentView(Some(&text));
            scroll
        },
        move |scroll, _| {
            if let Some(text) = scroll
                .documentView()
                .and_then(|text| text.downcast::<NSTextView>().ok())
                .filter(|text| text.isEditable() != editable)
            {
                text.setEditable(editable);
            }
        },
    )
}

pub(crate) fn where_of(id: &str) -> Option<String> {
    if crate::remote::is_remote(id) {
        let parent = crate::remote::parent(id);
        return crate::remote::is_remote(parent).then(|| parent.to_owned());
    }
    let parent = Path::new(id)
        .parent()
        .filter(|parent| parent.is_absolute())?;
    Some(parent.to_string_lossy().into_owned())
}

pub(crate) fn open(
    demo: &Entity<FinderDemo>,
    at: Option<&'static str>,
    summary: bool,
    cx: &mut App,
) {
    let lines: Vec<Line> = {
        let state = demo.read(cx);
        let chosen = match at {
            Some(_) => Vec::new(),
            None => state.chosen_lines(),
        };
        let at = at.or_else(|| (chosen.is_empty() && !state.results()).then(|| state.here()));
        match at {
            Some(id) => crate::locations::info_line(id)
                .or_else(|| crate::data::path_line(id))
                .into_iter()
                .collect(),
            None => chosen,
        }
    };
    let where_ = SharedString::from(
        lines
            .first()
            .and_then(|line| where_of(&line.key))
            .unwrap_or_default(),
    );
    let width = info_width();
    if lines
        .first()
        .is_some_and(|line| crate::remote::is_remote(&line.key))
    {
        let groups = if summary {
            vec![lines]
        } else {
            lines.into_iter().map(|line| vec![line]).collect()
        };
        for (step, lines) in groups.into_iter().enumerate() {
            let remote = Remote::new(lines, where_.clone());
            let shape = (width, remote.default_height(width));
            let title = intern(&remote.title());
            open_window(remote, step as f32 * 24., shape, Some(title), cx);
        }
        return;
    }
    if summary && lines.len() > 1 {
        let combined = Combined::new(lines, where_);
        let shape = (width, combined.default_height(width));
        open_window(combined, 0., shape, Some("Multiple Item Info"), cx);
        return;
    }
    for (step, line) in lines.into_iter().enumerate() {
        let info = Info::new(line, where_.clone(), demo.downgrade());
        let shape = (width, info.default_height(width));
        open_window(info, step as f32 * 24., shape, None, cx);
    }
}

fn open_window<V: Render>(
    view: V,
    offset: f32,
    (width, height): (f32, f32),
    title: Option<&'static str>,
    cx: &mut App,
) {
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(60. + offset), px(80. + offset)),
                size: size(px(width), px(height)),
            })),
            window_min_size: Some(size(px(NARROWEST), px(SHORTEST))),
            ..window_options()
        },
        move |window, cx| {
            if let Some(title) = title {
                window.set_window_title(title);
            }
            crate::chrome::toolbar::system_corners(window);
            let view = cx.new(|_| view);
            if let Some(ns) = ns_window(window) {
                ns.setTitleVisibility(NSWindowTitleVisibility::Visible);
                let observer = keep_fitted(&ns);
                cx.observe_release(&view, move |_, _| unsafe {
                    NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref());
                })
                .detach();
            }
            view
        },
    );
}

pub(crate) struct Combined {
    lines: Vec<Line>,
    where_: SharedString,
    weight: Option<Weight>,
    so_far: Option<Weight>,
    weighing: bool,
    general: Fold,
    same: Option<bool>,
    same_seen: Option<u64>,
}

impl Combined {
    pub(crate) fn new(lines: Vec<Line>, where_: SharedString) -> Self {
        Combined {
            lines,
            where_,
            weight: None,
            so_far: None,
            weighing: false,
            general: Fold::new(panes_open()[Pane::General as usize]),
            same: None,
            same_seen: None,
        }
    }

    fn fold(&mut self, cx: &mut Context<Self>) {
        self.general.toggle();
        remember(Pane::General, self.general.is_open());
        cx.notify();
    }

    fn weigh(&mut self, cx: &mut Context<Self>) {
        if self.weighing {
            return;
        }
        self.weighing = true;
        let paths = self.paths();
        weigh_in_background(
            cx,
            move |report| weigh_all(&paths, report),
            |combined, weight, done| {
                if done {
                    combined.weight = Some(weight);
                } else {
                    combined.so_far = Some(weight);
                }
            },
        );
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.lines
            .iter()
            .map(|line| PathBuf::from(line.row.path.as_ref()))
            .collect()
    }

    fn check_same(&mut self, cx: &mut Context<Self>) {
        let generation = icon_well::generation();
        if self.same_seen == Some(generation) {
            return;
        }
        self.same_seen = Some(generation);
        let paths = self.paths();
        in_background(
            cx,
            move || icon_well::same_icons(&paths),
            |combined, same| combined.same = Some(same),
        );
    }

    fn name(&self) -> String {
        items_text(self.lines.len() as u64)
    }

    fn rows(&self) -> Vec<(&'static str, SharedString)> {
        let mut kinds: Vec<&str> = self
            .lines
            .iter()
            .map(|line| line.row.kind.as_ref())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut rows = vec![("Kind", kinds.join(", ").into())];
        rows.extend(weight_rows(self.weight, self.so_far));
        if !self.where_.is_empty() {
            rows.push((WHERE, self.where_.clone()));
        }
        rows
    }

    fn default_height(&self, width: f32) -> f32 {
        let form = Form::new(width, 1.);
        let head = header_height(&form, &self.name(), true, false);
        TITLEBAR + head + SECTION + TITLE + values_height(&form, &self.rows()) + BOTTOM
    }
}

impl Render for Combined {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.weigh(cx);
        self.check_same(cx);
        let form = Form::new(
            f32::from(window.viewport_size().width),
            1. / window.scale_factor(),
        );
        let summary = self
            .weight
            .map(|weight| byte_string(weight.bytes))
            .unwrap_or_default();
        let this = cx.weak_entity();
        let (id, title) = Pane::General.section();
        let general = fold::section(
            &form,
            id,
            title,
            &self.general,
            move |_, cx| {
                this.update(cx, |combined, cx| combined.fold(cx)).ok();
            },
            vec![card("values", values(&form, &self.rows()))],
        );
        let paths = self.paths();
        let same = self.same == Some(true);
        let first = paths.first().filter(|_| same).cloned();
        let well = icon_well(
            paths,
            same,
            Some(move || match &first {
                Some(first) => icon_of(&first.to_string_lossy()),
                None => NSImage::initWithContentsOfFile(
                    NSImage::alloc(),
                    &NSString::from_str(MANY_ICON),
                ),
            }),
        );
        let preview = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(PREVIEW)).child(well));
        let head = header(&form, preview, &self.name(), &summary, None);
        if self.general.moving() {
            window.request_animation_frame();
        }
        page(
            window,
            vec![head, general.into_any_element()],
            BOTTOM,
            bound,
        )
    }
}

pub(crate) struct Remote {
    lines: Vec<Line>,
    where_: SharedString,
    stat: Option<crate::remote::Stat>,
    asked: bool,
    folds: [Fold; 6],
}

impl Remote {
    pub(crate) fn new(lines: Vec<Line>, where_: SharedString) -> Self {
        Remote {
            lines,
            where_,
            stat: None,
            asked: false,
            folds: panes_open().map(Fold::new),
        }
    }

    pub(crate) fn title(&self) -> String {
        match self.lines.as_slice() {
            [line] => format!("{} Info", line.row.display),
            _ => "Multiple Item Info".to_owned(),
        }
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let [line] = self.lines.as_slice() else {
            return;
        };
        if self.asked {
            return;
        }
        self.asked = true;
        let id = line.key.clone();
        in_background(
            cx,
            move || crate::remote::stat(&id),
            |remote, stat| remote.stat = stat,
        );
    }

    fn name(&self) -> String {
        match self.lines.as_slice() {
            [line] => line.row.display.to_string(),
            lines => items_text(lines.len() as u64),
        }
    }

    fn bytes(&self) -> Option<u64> {
        self.lines
            .iter()
            .map(|line| line.row.opens.is_none().then_some(line.row.bytes))
            .sum()
    }

    fn rows(&self, form: &Form) -> Vec<(&'static str, SharedString)> {
        let mut kinds: Vec<&str> = self
            .lines
            .iter()
            .map(|line| line.row.kind.as_ref())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        let mut rows = vec![("Kind", kinds.join(", ").into())];
        rows.extend(self.bytes().map(|bytes| ("Size", size_text(bytes).into())));
        if !self.where_.is_empty() {
            rows.push((WHERE, self.where_.clone()));
        }
        let modified = match (&self.stat, self.lines.as_slice()) {
            (Some(stat), _) => stat.modified,
            (None, [line]) => line.row.modified_at,
            _ => f64::NAN,
        };
        if !modified.is_nan() {
            let room = form.room("Modified");
            rows.push(("Modified", long_date(modified, room).into()));
        }
        rows
    }

    fn access(&self, form: &Form) -> Option<Vec<(f32, Part)>> {
        let stat = self.stat.as_ref()?;
        let mode = stat.mode?;
        let rwx: String = (0..9)
            .rev()
            .map(|bit| {
                if mode >> bit & 1 == 1 {
                    char::from(b"xwr"[bit % 3])
                } else {
                    '-'
                }
            })
            .collect();
        let mut parts = Vec::new();
        if stat.mine == Some(true) {
            let (read, write) = (mode & 0o400 != 0, mode & 0o200 != 0);
            let yours = if read && write {
                Privilege::ReadWrite
            } else if read {
                Privilege::ReadOnly
            } else if write {
                Privilege::WriteOnly
            } else {
                Privilege::NoAccess
            };
            let yours = div()
                .px(px(MARGIN + INSET))
                .child(native_label("yours", yours_text(yours)).color(SystemColor::SecondaryLabel));
            parts.push((4., Part::Plain(yours.into_any_element())));
        }
        let rows = [("Permissions", format!("{rwx} ({mode:03o})").into())];
        parts.push(card("values", values(form, &rows)));
        Some(parts)
    }

    fn default_height(&self, width: f32) -> f32 {
        let form = Form::new(width, 1.);
        let summary = self.bytes().is_some();
        let head = header_height(&form, &self.name(), summary, false);
        TITLEBAR + head + SECTION + TITLE + values_height(&form, &self.rows(&form)) + BOTTOM
    }

    fn pane(
        &self,
        form: &Form,
        this: &WeakEntity<Self>,
        pane: Pane,
        parts: Vec<(f32, Part)>,
    ) -> AnyElement {
        let this = this.clone();
        let toggle = move |_: &mut Window, cx: &mut App| {
            this.update(cx, |remote, cx| {
                let fold = &mut remote.folds[pane as usize];
                fold.toggle();
                remember(pane, fold.is_open());
                cx.notify();
            })
            .ok();
        };
        let (id, title) = pane.section();
        fold::section(form, id, title, &self.folds[pane as usize], toggle, parts).into_any_element()
    }
}

impl Render for Remote {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ask(cx);
        let scale = window.scale_factor();
        let form = Form::new(f32::from(window.viewport_size().width), 1. / scale);
        let this = cx.weak_entity();
        let preview = match self.lines.as_slice() {
            [line] => icon_view(&line.row, scale),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(PREVIEW)).child(icon_file(MANY_ICON)))
                .into_any_element(),
        };
        let summary = self.bytes().map(byte_string).unwrap_or_default();
        let general = vec![card("values", values(&form, &self.rows(&form)))];
        let mut content = vec![
            header(&form, preview, &self.name(), &summary, None),
            self.pane(&form, &this, Pane::General, general),
        ];
        if let Some(parts) = self.access(&form) {
            content.push(self.pane(&form, &this, Pane::Privileges, parts));
        }
        if self.folds.iter().any(Fold::moving) {
            window.request_animation_frame();
        }
        page(window, content, BOTTOM, bound)
    }
}
