use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{LazyLock, Mutex, OnceLock};

use gpui::{AnyElement, Context, Hsla, SharedString, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor, system_color};
use objc2_app_kit::{NSColor, NSColorSpace};
use objc2_core_foundation::CGFloat;

use crate::app::FinderDemo;

const TAG_DOT: f32 = 10.;
const TAG_DOT_PITCH: f32 = 5.;
const TAG_DOTS_MAX: usize = 3;
const TAG_LIGHTEN: f32 = 0.2;
const TAG_PILL_ALPHA: f32 = 0.85;
const KNOWN_MAX: usize = 20_000;

#[derive(Clone, PartialEq)]
pub(crate) struct RowTag {
    pub(crate) name: SharedString,
    pub(crate) color: u8,
}

pub(crate) fn tags() -> &'static [finder_fs::tags::Tag] {
    static TAGS: OnceLock<Vec<finder_fs::tags::Tag>> = OnceLock::new();
    TAGS.get_or_init(finder_fs::tags::favorites)
}

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

fn finder_edge(color: u8) -> Hsla {
    tag_system_color(color).map_or(gpui::white(), system_color)
}

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

pub(crate) fn tag_pill(tag: &RowTag, text: Hsla) -> AnyElement {
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

pub(crate) fn tag_dot(color: u8) -> AnyElement {
    let dot = div()
        .w(px(TAG_DOT))
        .h(px(TAG_DOT))
        .rounded_full()
        .flex_none()
        .border(px(1.));
    match tag_system_color(color) {
        Some(_) => dot.border_color(finder_edge(color)).bg(finder_color(color)),
        None => dot.border_color(system_color(SystemColor::SecondaryLabel)),
    }
    .into_any_element()
}

fn dot_colors(tags: &[RowTag]) -> impl Iterator<Item = u8> + '_ {
    tags.iter()
        .map(|tag| tag.color)
        .filter(|&color| tag_system_color(color).is_some())
}

fn dots_w(shown: usize) -> f32 {
    match shown {
        0 => 0.,
        shown => TAG_DOT + TAG_DOT_PITCH * (shown - 1) as f32,
    }
}

pub(crate) fn tag_dots_w(tags: &[RowTag]) -> f32 {
    dots_w(dot_colors(tags).count().min(TAG_DOTS_MAX))
}

pub(crate) fn tag_dots(tags: &[RowTag]) -> Option<AnyElement> {
    let colored = dot_colors(tags).count();
    let shown = colored.min(TAG_DOTS_MAX);
    (shown > 0).then(|| {
        div()
            .relative()
            .flex_none()
            .w(px(dots_w(shown)))
            .h(px(TAG_DOT))
            .children(
                dot_colors(tags)
                    .skip(colored - shown)
                    .enumerate()
                    .map(|(at, color)| {
                        div()
                            .absolute()
                            .left(px((shown - 1 - at) as f32 * TAG_DOT_PITCH))
                            .child(tag_dot(color))
                    }),
            )
            .into_any_element()
    })
}

pub(crate) fn tag_title(id: &str) -> String {
    if id == crate::search::RECENTS {
        return finder_fs::words::say("Recents").into_owned();
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

/// Reads the disk: background work only. A listed tag wears Finder's colour, not the file's.
fn read_row_tags(path: &str) -> Vec<RowTag> {
    finder_fs::tags::of(Path::new(path))
        .into_iter()
        .map(
            |tag| match all().iter().find(|listed| listed.name == tag.name) {
                Some(listed) => RowTag {
                    name: SharedString::new_static(listed.name.as_str()),
                    color: listed.color,
                },
                None => RowTag {
                    name: tag.name.into(),
                    color: tag.color,
                },
            },
        )
        .collect()
}

struct Held {
    tags: Vec<RowTag>,
    stale: bool,
}

/// Each path's tags by folder; a change marks them stale, still shown until the next read lands.
#[derive(Default)]
struct Known {
    dirs: HashMap<PathBuf, HashMap<String, Held>>,
    order: VecDeque<PathBuf>,
    held: usize,
}

fn split(path: &str) -> (&Path, &str) {
    let at = Path::new(path);
    match (at.parent(), at.file_name().and_then(|name| name.to_str())) {
        (Some(dir), Some(name)) => (dir, name),
        _ => (Path::new(""), path),
    }
}

impl Known {
    fn get_mut(&mut self, path: &str) -> Option<&mut Held> {
        let (dir, name) = split(path);
        self.dirs.get_mut(dir)?.get_mut(name)
    }

    fn insert(&mut self, path: String, tags: Vec<RowTag>) {
        let (dir, name) = split(&path);
        let names = match self.dirs.entry(dir.to_path_buf()) {
            Entry::Occupied(names) => names.into_mut(),
            Entry::Vacant(room) => {
                self.order.push_back(dir.to_path_buf());
                room.insert(HashMap::new())
            }
        };
        if names
            .insert(name.to_owned(), Held { tags, stale: false })
            .is_none()
        {
            self.held += 1;
        }
        for _ in 0..self.order.len() {
            if self.held <= KNOWN_MAX {
                break;
            }
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if oldest == dir {
                self.order.push_back(oldest);
            } else if let Some(names) = self.dirs.remove(&oldest) {
                self.held -= names.len();
            }
        }
    }

    fn stale(&mut self, dir: &Path) {
        for held in self
            .dirs
            .get_mut(dir)
            .into_iter()
            .flat_map(HashMap::values_mut)
        {
            held.stale = true;
        }
    }
}

static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);
static LANDED: AtomicU64 = AtomicU64::new(0);
static READER: LazyLock<Mutex<Sender<String>>> = LazyLock::new(|| {
    let (send, paths) = channel::<String>();
    std::thread::spawn(move || {
        for path in paths {
            let tags = read_row_tags(&path);
            KNOWN.lock().unwrap().insert(path, tags);
            LANDED.fetch_add(1, Ordering::Release);
            crate::read::EVENTS.ring();
        }
    });
    Mutex::new(send)
});

/// Never reads the disk, which a stalled share would hang; an unread row is untagged till it lands.
pub(crate) fn row_tags(path: &str) -> Vec<RowTag> {
    let mut known = KNOWN.lock().unwrap();
    let tags = match known.get_mut(path) {
        Some(held) if !held.stale => return held.tags.clone(),
        Some(held) => {
            held.stale = false;
            held.tags.clone()
        }
        None => {
            known.insert(path.to_owned(), Vec::new());
            Vec::new()
        }
    };
    let _ = READER.lock().unwrap().send(path.to_owned());
    tags
}

pub(crate) fn tags_landed() -> u64 {
    LANDED.load(Ordering::Acquire)
}

pub(crate) fn tags_changed_in(dir: &Path) {
    KNOWN.lock().unwrap().stale(dir);
}

impl FinderDemo {
    pub(crate) fn tag_selection(&mut self, tag: usize, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .selected
            .snapshot()
            .iter()
            .map(|key| PathBuf::from(key.as_ref()))
            .collect();
        let wanted = tags()[tag].clone();
        self.retag(
            paths,
            move |paths| {
                finder_fs::tags::toggle(paths, &wanted).ok();
            },
            cx,
        );
    }

    pub(crate) fn retag(
        &self,
        paths: Vec<PathBuf>,
        write: impl FnOnce(&[PathBuf]) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.background_executor()
            .spawn(async move {
                write(&paths);
                for path in paths {
                    let key = path.to_string_lossy().into_owned();
                    let tags = read_row_tags(&key);
                    KNOWN.lock().unwrap().insert(key, tags);
                }
                LANDED.fetch_add(1, Ordering::Release);
                crate::read::EVENTS.ring();
            })
            .detach();
    }
}
