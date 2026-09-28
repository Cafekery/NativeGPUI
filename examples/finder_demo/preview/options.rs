use std::cell::Cell;

use gpui::{
    App, Bounds, Context, EntityId, Render, SharedString, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{CheckState, native_checkbox, native_root, native_rule, window_options};
use objc2_app_kit::NSWindowStyleMask;

use crate::app::FinderDemo;
use crate::data::{Face, Row};
use crate::frame::ns_window;

/// Where a preview row's value comes from.
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
    /// What the choice is saved under: two rows may share a label.
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

/// Finder's Preview Options for one kind of item.
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

/// Spotlight's display and preview attributes for audio, grouped as Finder groups a movie's.
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
                Field {
                    label: "Sample rate",
                    source: Source::Attr("kMDItemAudioSampleRate"),
                    on: true,
                },
                Field {
                    label: "Bits per sample",
                    source: Source::Attr("kMDItemBitsPerSample"),
                    on: true,
                },
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

/// Not read off Finder: the rows the card showed an app before these options.
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

/// Counts every choice made, so a card drawn from the old ones is drawn again.
pub(crate) fn changed() -> u64 {
    CHANGED.get()
}

/// Whether `key` shows for `kind`: the user's choice, else Finder's.
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
        .chain(kind.groups.iter().flat_map(|group| match group.fields {
            [] => vec![group.title],
            fields => fields.iter().map(Field::key).collect(),
        }))
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

pub(crate) struct PreviewOptions {
    kind: &'static Kind,
}

fn state(kind: &Kind, keys: &[&str]) -> CheckState {
    let shown = keys.iter().filter(|key| shows(kind, key)).count();
    match shown {
        0 => CheckState::Off,
        n if n == keys.len() => CheckState::On,
        _ => CheckState::Mixed,
    }
}

impl Render for PreviewOptions {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        let small = tokens::small_system_font_size();
        let checkbox = |id: SharedString, title: &'static str, keys: Vec<&'static str>| {
            let checked = state(kind, &keys);
            native_checkbox(id, checked == CheckState::On)
                .title(title)
                .state(checked)
                .on_change(move |state: &CheckState, _, cx| {
                    choose(kind, &keys, *state != CheckState::Off, cx);
                })
        };
        let mut groups = div().flex().flex_col().gap(px(6.)).child(checkbox(
            "ng.finder.preview.options.tags".into(),
            TAGS,
            vec![TAGS],
        ));
        for group in kind.groups {
            let keys: Vec<&'static str> = match group.fields {
                [] => vec![group.title],
                fields => fields.iter().map(Field::key).collect(),
            };
            groups = groups
                .child(checkbox(
                    format!("ng.finder.preview.options.{}", group.title).into(),
                    group.title,
                    keys,
                ))
                .children(group.fields.iter().enumerate().map(|(at, field)| {
                    div().pl(px(20.)).child(checkbox(
                        format!("ng.finder.preview.options.{}.{at}", group.title).into(),
                        field.label,
                        vec![field.key()],
                    ))
                }));
        }
        native_root(
            div()
                .size_full()
                .bg(system_color(SystemColor::WindowBackground))
                .pt(px(33.))
                .px(px(20.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(small)
                        .line_height(px(16.))
                        .text_color(system_color(SystemColor::Label))
                        .mb(px(7.))
                        .child("Show Info:"),
                )
                .child(groups)
                .child(
                    div()
                        .mt(px(12.))
                        .h(px(5.))
                        .child(native_rule("ng.finder.preview.options.rule")),
                )
                .child(div().mt(px(20.)).child(checkbox(
                    "ng.finder.preview.options.quick".into(),
                    QUICK_ACTIONS,
                    vec![QUICK_ACTIONS],
                ))),
        )
    }
}

pub(crate) fn is_open() -> bool {
    OPEN.get().is_some()
}

/// Show Preview Options and Hide Preview Options.
pub(crate) fn toggle(demo: &FinderDemo, cx: &mut Context<FinderDemo>) {
    match OPEN.take() {
        Some(open) => cx.defer(move |cx| {
            open.update(cx, |_, window, _| window.remove_window()).ok();
        }),
        None => {
            let kind = subject(demo);
            FOLLOWED.set(Some(followed(demo, cx)));
            cx.defer(move |cx| show(kind, cx));
        }
    }
}

/// The panel is for the kind the key Finder window's cursor is on.
pub(crate) fn follow(demo: &FinderDemo, window: &Window, cx: &mut Context<FinderDemo>) {
    if OPEN.get().is_none() || !window.is_window_active() {
        return;
    }
    let now = followed(demo, cx);
    if FOLLOWED.get() == Some(now) {
        return;
    }
    FOLLOWED.set(Some(now));
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

/// Finder's rows sit 24 pt apart.
fn height(kind: &Kind) -> f32 {
    let rows = 1 + kind
        .groups
        .iter()
        .map(|group| 1 + group.fields.len())
        .sum::<usize>();
    119. + 24. * rows as f32
}

fn show(kind: &'static Kind, cx: &mut App) {
    let title = format!("Preview Options - {}", kind.name);
    let tall = height(kind);
    if let Some(open) = OPEN.get() {
        let shown = open.update(cx, |panel, window, cx| {
            window.set_window_title(&title);
            let mut frame = window.bounds();
            frame.size.height = px(tall);
            window.resize(frame.size);
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
                size: size(px(240.), px(tall)),
            })),
            focus: false,
            is_minimizable: false,
            is_resizable: false,
            ..window_options()
        },
        |window, cx| {
            window.set_window_title(&title);
            if let Some(panel) = ns_window(window) {
                panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::UtilityWindow);
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
