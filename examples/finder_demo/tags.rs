use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gpui::{AnyElement, Context, Hsla, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor, system_color};
use objc2_app_kit::{NSColor, NSColorSpace};
use objc2_core_foundation::CGFloat;

use crate::app::FinderDemo;

const TAG_DOT: f32 = 10.;
const TAG_DOT_PITCH: f32 = 5.;
const TAG_DOTS_MAX: usize = 3;
const TAG_LIGHTEN: f32 = 0.2;
const TAG_PILL_ALPHA: f32 = 0.85;

/// The tags Finder's own tag menus offer, read once from Finder's preferences.
pub(crate) fn tags() -> &'static [finder_fs::tags::Tag] {
    static TAGS: OnceLock<Vec<finder_fs::tags::Tag>> = OnceLock::new();
    TAGS.get_or_init(finder_fs::tags::favorites)
}

/// Reading every tag runs a query against Finder's key value store, so it is read once and warmed
/// off the main thread.
pub(crate) fn all() -> &'static [finder_fs::tags::Tag] {
    static ALL: OnceLock<Vec<finder_fs::tags::Tag>> = OnceLock::new();
    ALL.get_or_init(finder_fs::tags::all)
}

fn tag_system_color(color: u8) -> Option<SystemColor> {
    Some(match color {
        1 => SystemColor::SystemGray,
        2 => SystemColor::SystemGreen,
        3 => SystemColor::SystemPurple,
        4 => SystemColor::SystemBlue,
        5 => SystemColor::SystemYellow,
        6 => SystemColor::SystemRed,
        7 => SystemColor::SystemOrange,
        _ => return None,
    })
}

pub(crate) fn tag_ns_color(color: u8) -> Option<objc2::rc::Retained<NSColor>> {
    tag_system_color(color).map(|system| system.ns_color())
}

pub(crate) fn finder_edge(color: u8) -> Hsla {
    tag_system_color(color).map_or(gpui::white(), system_color)
}

/// Finder's dot fill: its system colour lifted toward white on the display's own primaries.
pub(crate) fn finder_color(color: u8) -> Hsla {
    let Some(system) = tag_system_color(color) else {
        return gpui::white();
    };
    let space = NSColorSpace::displayP3ColorSpace();
    let lightened = system
        .ns_color()
        .colorUsingColorSpace(&space)
        .map(|wide| {
            let lift = |channel: CGFloat| channel + (1. - channel) * TAG_LIGHTEN as CGFloat;
            NSColor::colorWithDisplayP3Red_green_blue_alpha(
                lift(wide.redComponent()),
                lift(wide.greenComponent()),
                lift(wide.blueComponent()),
                1.,
            )
        })
        .and_then(|color| tokens::srgb(&color));
    lightened.unwrap_or_else(|| system_color(system))
}

pub(crate) fn finder_color_rgb(color: u8) -> Option<(u8, u8, u8)> {
    tag_system_color(color)?;
    let rgba = gpui::Rgba::from(finder_color(color));
    let byte = |channel: f32| (channel * 255.).round().clamp(0., 255.) as u8;
    Some((byte(rgba.r), byte(rgba.g), byte(rgba.b)))
}

pub(crate) fn tag_index(name: &str) -> Option<usize> {
    tags().iter().position(|tag| tag.name == name)
}

pub(crate) fn finder_tag(index: usize) -> finder_fs::tags::Tag {
    tags()[index].clone()
}

pub(crate) fn tag_pill(index: usize, text: Hsla) -> AnyElement {
    let tag = &tags()[index];
    let mut wash = finder_edge(tag.color);
    wash.a = TAG_PILL_ALPHA;
    div()
        .px(px(6.))
        .rounded(px(3.))
        .bg(wash)
        .text_color(text)
        .child(tag.name.clone())
        .into_any_element()
}

pub(crate) fn tag_dot(index: usize) -> AnyElement {
    let color = tags()[index].color;
    div()
        .w(px(TAG_DOT))
        .h(px(TAG_DOT))
        .rounded_full()
        .flex_none()
        .border(px(1.))
        .border_color(finder_edge(color))
        .bg(finder_color(color))
        .into_any_element()
}

pub(crate) fn tag_dots_w(count: usize) -> f32 {
    match count.min(TAG_DOTS_MAX) {
        0 => 0.,
        shown => TAG_DOT + TAG_DOT_PITCH * (shown - 1) as f32,
    }
}

/// Finder stacks the newest tag leftmost and over the ones before it, three dots at most.
pub(crate) fn tag_dots(tags: &[usize]) -> Option<AnyElement> {
    let shown = &tags[tags.len().saturating_sub(TAG_DOTS_MAX)..];
    if shown.is_empty() {
        return None;
    }
    Some(
        div()
            .relative()
            .flex_none()
            .w(px(tag_dots_w(shown.len())))
            .h(px(TAG_DOT))
            .children(shown.iter().enumerate().map(|(at, tag)| {
                div()
                    .absolute()
                    .left(px((shown.len() - 1 - at) as f32 * TAG_DOT_PITCH))
                    .child(tag_dot(*tag))
            }))
            .into_any_element(),
    )
}

pub(crate) fn tag_title(id: &str) -> String {
    if id == crate::search::RECENTS {
        return "Recents".to_string();
    }
    if let Some(file) = crate::smart::file_of(id) {
        let stem = Path::new(file).file_stem().unwrap_or_default();
        return stem.to_string_lossy().replace(':', "/");
    }
    match id.strip_prefix("tag:").unwrap_or_default() {
        "" => "All Tags".to_string(),
        name => name.to_string(),
    }
}

pub(crate) fn row_tags(path: &str) -> Vec<usize> {
    finder_fs::tags::of(Path::new(path))
        .iter()
        .filter_map(|tag| tag_index(&tag.name))
        .collect()
}

impl FinderDemo {
    pub(crate) fn tag_selection(&mut self, tag: usize, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .selected
            .snapshot()
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        let wanted = finder_tag(tag);
        cx.spawn(async move |this, cx| {
            let carried = cx
                .background_executor()
                .spawn(async move {
                    finder_fs::tags::toggle(&paths, &wanted).ok();
                    paths
                        .iter()
                        .map(|path| {
                            let key = path.to_string_lossy().into_owned();
                            let tags = row_tags(&key);
                            (key, tags)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                for (key, tags) in carried {
                    this.tags.insert(key.into(), tags);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
