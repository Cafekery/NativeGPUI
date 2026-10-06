use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, EntityId, Render, Window, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, point, prelude::*, px, size,
};
use native_gpui::window_options;
use objc2_app_kit::{NSWindowStyleMask, NSWindowTitleVisibility};

use crate::app::FinderDemo;
use crate::data::{Face, Row};
use crate::form::{self, Flip, Sheet, switch_row};
use crate::frame::ns_window;

const WIDTH: f32 = 280.;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Source {
    Created,
    Modified,
    LastOpened,
    ContentCreated,
    Dimensions,
    PageSize,
    Dpi,
    Duration,
    Attr(&'static str),
}

pub(crate) struct Field {
    pub(crate) label: &'static str,
    pub(crate) source: Source,
    on: bool,
}

impl Field {
    /// Two rows may share a label.
    pub(crate) fn key(&self) -> &'static str {
        match self.source {
            Source::Attr(name) => name,
            _ => self.label,
        }
    }
}

pub(crate) struct Group {
    pub(crate) title: &'static str,
    pub(crate) fields: &'static [Field],
}

impl Group {
    fn keys(&self) -> Vec<&'static str> {
        match self.fields {
            [] => vec![self.title],
            fields => fields.iter().map(Field::key).collect(),
        }
    }
}

pub(crate) struct Kind {
    pub(crate) name: &'static str,
    pub(crate) groups: &'static [Group],
}

const fn on(label: &'static str, source: Source) -> Field {
    Field {
        label,
        source,
        on: true,
    }
}

const fn off(label: &'static str, source: Source) -> Field {
    Field {
        label,
        source,
        on: false,
    }
}

const fn attr(label: &'static str, name: &'static str) -> Field {
    off(label, Source::Attr(name))
}

pub(crate) const TAGS: &str = "Tags";
pub(crate) const QUICK_ACTIONS: &str = "Show Quick Actions";

const DATES: Group = Group {
    title: "Dates",
    fields: &[
        on("Created", Source::Created),
        on("Modified", Source::Modified),
        on("Last opened", Source::LastOpened),
    ],
};

const KEYWORDS: Field = attr("Keywords", "kMDItemKeywords");
const WHERE_FROM: Field = attr("Where from", "kMDItemWhereFroms");

const IMAGE: Kind = Kind {
    name: "Image",
    groups: &[
        DATES,
        Group {
            title: "Image Detail",
            fields: &[
                off("Content created", Source::ContentCreated),
                on("Dimensions", Source::Dimensions),
                on("Resolution", Source::Dpi),
                attr("Color space", "kMDItemColorSpace"),
                attr("Color profile", "kMDItemProfileName"),
            ],
        },
        Group {
            title: "Exif",
            fields: &[
                attr("Device make", "kMDItemAcquisitionMake"),
                attr("Device model", "kMDItemAcquisitionModel"),
                attr("Lens model", "kMDItemLensModel"),
                attr("Aperture value", "kMDItemAperture"),
                attr("Max aperture", "kMDItemMaxAperture"),
                attr("Exposure time", "kMDItemExposureTimeSeconds"),
                attr("Focal length", "kMDItemFocalLength"),
                attr("ISO speed", "kMDItemISOSpeed"),
                attr("Flash", "kMDItemFlashOnOff"),
                attr("F number", "kMDItemFNumber"),
                attr("Exposure program", "kMDItemExposureProgram"),
                attr("Metering mode", "kMDItemMeteringMode"),
                attr("White balance", "kMDItemWhiteBalance"),
                attr("Content Creator", "kMDItemXMPDigitalSourceType"),
                attr("Credit", "kMDItemXMPCredit"),
                attr("Content Creator", "kMDItemCreator"),
            ],
        },
        Group {
            title: "Location",
            fields: &[],
        },
    ],
};

const TEXT: Kind = Kind {
    name: "Text",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[KEYWORDS, WHERE_FROM],
        },
        Group {
            title: "More",
            fields: &[
                attr("Title", "kMDItemTitle"),
                attr("Subject", "kMDItemSubject"),
                attr("Organizations", "kMDItemOrganizations"),
                attr("Authors", "kMDItemAuthors"),
                attr("Editors", "kMDItemEditors"),
                attr("Comment", "kMDItemComment"),
            ],
        },
    ],
};

const PDF: Kind = Kind {
    name: "PDF document",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[
                KEYWORDS,
                WHERE_FROM,
                attr("Title", "kMDItemTitle"),
                attr("Authors", "kMDItemAuthors"),
                attr("Description", "kMDItemDescription"),
                attr("Version", "kMDItemVersion"),
                attr("Pages", "kMDItemNumberOfPages"),
                off("Resolution", Source::PageSize),
                attr("Security", "kMDItemSecurityMethod"),
                attr("Content Creator", "kMDItemCreator"),
                attr("Encoding software", "kMDItemEncodingApplications"),
            ],
        },
    ],
};

const MOVIE: Kind = Kind {
    name: "Movie",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[
                KEYWORDS,
                WHERE_FROM,
                on("Dimensions", Source::Dimensions),
                attr("Codecs", "kMDItemCodecs"),
                attr("Media extensions", "kMDItemMediaExtensions"),
                attr("Color profile", "kMDItemProfileName"),
                attr("Location", "kMDItemNamedLocation"),
                attr("City", "kMDItemCity"),
                attr("State or Province", "kMDItemStateOrProvince"),
                attr("Region", "kMDItemCountry"),
                attr("Latitude", "kMDItemLatitude"),
                attr("Longitude", "kMDItemLongitude"),
                attr("Description", "kMDItemDescription"),
                attr("Title", "kMDItemTitle"),
                on("Duration", Source::Duration),
                attr("Authors", "kMDItemAuthors"),
                attr("Performers", "kMDItemPerformers"),
                attr("Director", "kMDItemDirector"),
                attr("Producer", "kMDItemProducer"),
                attr("Genre", "kMDItemGenre"),
                attr("Copyright", "kMDItemCopyright"),
                attr("Audio channels", "kMDItemAudioChannelCount"),
                attr("Sample rate", "kMDItemAudioSampleRate"),
                attr("Bits per sample", "kMDItemBitsPerSample"),
                attr("Encoding software", "kMDItemEncodingApplications"),
                attr("Original Format", "kMDItemOriginalFormat"),
                attr("Original Source", "kMDItemOriginalSource"),
                attr("Rating", "kMDItemStarRating"),
                attr("Movie information", "kMDItemInformation"),
            ],
        },
    ],
};

const AUDIO: Kind = Kind {
    name: "Audio",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[
                KEYWORDS,
                WHERE_FROM,
                attr("Title", "kMDItemTitle"),
                on("Duration", Source::Duration),
                attr("Authors", "kMDItemAuthors"),
                attr("Performers", "kMDItemPerformers"),
                attr("Genre", "kMDItemGenre"),
                attr("Copyright", "kMDItemCopyright"),
                attr("Audio channels", "kMDItemAudioChannelCount"),
                on("Sample rate", Source::Attr("kMDItemAudioSampleRate")),
                on("Bits per sample", Source::Attr("kMDItemBitsPerSample")),
                attr("Codecs", "kMDItemCodecs"),
                attr("Encoding software", "kMDItemEncodingApplications"),
                attr("Album", "kMDItemAlbum"),
                attr("Composer", "kMDItemComposer"),
                attr("Year recorded", "kMDItemRecordingYear"),
                attr("Comment", "kMDItemComment"),
            ],
        },
    ],
};

const FOLDER: Kind = Kind {
    name: "Folder",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[KEYWORDS, WHERE_FROM],
        },
    ],
};

const APPLICATION: Kind = Kind {
    name: "Application",
    groups: &[
        DATES,
        Group {
            title: "General",
            fields: &[
                KEYWORDS,
                WHERE_FROM,
                on("Version", Source::Attr("kMDItemVersion")),
                attr("Category", "kMDItemAppStoreCategory"),
            ],
        },
    ],
};

const DOCUMENT: Kind = Kind {
    name: "Document",
    groups: FOLDER.groups,
};

pub(crate) fn kind_of(row: &Row) -> &'static Kind {
    match row.face {
        Face::Folder | Face::Volume => &FOLDER,
        Face::Image => &IMAGE,
        Face::Movie => &MOVIE,
        Face::Audio => &AUDIO,
        Face::Pdf => &PDF,
        Face::Text | Face::Code => &TEXT,
        Face::Program => &APPLICATION,
        _ => &DOCUMENT,
    }
}

thread_local! {
    static CHANGED: Cell<u64> = const { Cell::new(0) };
    static OPEN: Cell<Option<WindowHandle<PreviewOptions>>> = const { Cell::new(None) };
    static FOLLOWED: Cell<Option<(EntityId, u64, &'static str)>> = const { Cell::new(None) };
}

pub(crate) fn changed() -> u64 {
    CHANGED.get()
}

pub(crate) fn shows(kind: &Kind, key: &str) -> bool {
    match crate::prefs::preview_options(kind.name) {
        Some(chosen) => chosen.iter().any(|on| on == key),
        None => {
            key == TAGS
                || key == QUICK_ACTIONS
                || kind
                    .groups
                    .iter()
                    .flat_map(|group| group.fields)
                    .any(|field| field.key() == key && field.on)
        }
    }
}

fn choose(kind: &'static Kind, keys: &[&str], shown: bool, cx: &mut App) {
    let on: Vec<&str> = [TAGS, QUICK_ACTIONS]
        .into_iter()
        .chain(kind.groups.iter().flat_map(Group::keys))
        .filter(|key| match keys.contains(key) {
            true => shown,
            false => shows(kind, key),
        })
        .collect();
    crate::prefs::set_preview_options(kind.name, &on);
    CHANGED.set(CHANGED.get() + 1);
    crate::read::notify_windows(cx);
    if let Some(open) = OPEN.get() {
        open.update(cx, |_, _, cx| cx.notify()).ok();
    }
}

struct PreviewOptions {
    kind: &'static Kind,
}

fn flip(kind: &'static Kind, keys: Vec<&'static str>) -> Flip {
    Rc::new(move |on, cx| choose(kind, &keys, on, cx))
}

/// A group's switch is on while any of its fields shows, and its fields can change only then.
fn sheet(kind: &'static Kind, width: f32, hair: f32) -> Sheet {
    let alone = |key: &'static str| {
        form::Row::new(switch_row(
            key,
            shows(kind, key),
            Some(flip(kind, vec![key])),
        ))
    };
    let mut rows = vec![alone(TAGS)];
    for group in kind.groups {
        let keys = group.keys();
        let on = keys.iter().any(|key| shows(kind, key));
        rows.push(form::Row::new(switch_row(
            group.title,
            on,
            Some(flip(kind, keys)),
        )));
        rows.extend(group.fields.iter().map(|field| {
            let key = field.key();
            let change = on.then(|| flip(kind, vec![key]));
            form::Row::new(switch_row(field.label, shows(kind, key), change)).nested()
        }));
    }
    let mut sheet = Sheet::new(width, hair);
    sheet.card("ng.finder.preview.options.info", Some("Show Info"), rows);
    sheet.card(
        "ng.finder.preview.options.quick",
        None,
        vec![alone(QUICK_ACTIONS)],
    );
    sheet
}

impl Render for PreviewOptions {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        sheet(self.kind, width, 1. / window.scale_factor()).into_page(window)
    }
}

pub(crate) fn is_open() -> bool {
    OPEN.get().is_some()
}

pub(crate) fn toggle(demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    match OPEN.take() {
        Some(open) => cx.defer(move |cx| {
            open.update(cx, |_, window, _| window.remove_window()).ok();
        }),
        None => show_for(demo, cx),
    }
}

pub(crate) fn close(cx: &mut App) {
    if let Some(open) = OPEN.take() {
        open.update(cx, |_, window, _| window.remove_window()).ok();
    }
}

pub(crate) fn follow(demo: &FinderDemo, window: &Window, cx: &mut Context<FinderDemo>) {
    if OPEN.get().is_none() || !window.is_window_active() {
        return;
    }
    if FOLLOWED.get() != Some(followed(demo, cx)) {
        show_for(demo, cx);
    }
}

fn show_for(demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    FOLLOWED.set(Some(followed(demo, cx)));
    let kind = subject(demo);
    cx.defer(move |cx| show(kind, cx));
}

fn followed(demo: &FinderDemo, cx: &Context<FinderDemo>) -> (EntityId, u64, &'static str) {
    (cx.entity_id(), demo.selected.rev(), demo.here())
}

fn subject(demo: &FinderDemo) -> &'static Kind {
    demo.cursor_line()
        .map_or(&FOLDER, |line| kind_of(&line.row))
}

fn height(kind: &'static Kind) -> f32 {
    sheet(kind, WIDTH, 1.).height()
}

fn show(kind: &'static Kind, cx: &mut App) {
    let title = format!("Preview Options - {}", kind.name);
    let tall = height(kind);
    if let Some(open) = OPEN.get() {
        let shown = open.update(cx, |panel, window, cx| {
            window.set_window_title(&title);
            window.resize(size(window.bounds().size.width, px(tall)));
            panel.kind = kind;
            cx.notify();
        });
        if shown.is_ok() {
            return;
        }
    }
    let opened = cx.open_window(
        WindowOptions {
            kind: WindowKind::Floating,
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(736.), px(310.)),
                size: size(px(WIDTH), px(tall)),
            })),
            is_minimizable: false,
            is_resizable: false,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title(&title);
            if let Some(panel) = ns_window(window) {
                panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::UtilityWindow);
                panel.setTitleVisibility(NSWindowTitleVisibility::Visible);
                crate::chrome::toolbar::system_corners_of(&panel);
            }
            let id = window.window_handle().window_id();
            cx.new(|cx| {
                cx.on_release(move |_, _| {
                    if OPEN.get().is_some_and(|open| open.window_id() == id) {
                        OPEN.set(None);
                    }
                })
                .detach();
                PreviewOptions { kind }
            })
        },
    );
    OPEN.set(opened.ok());
}
