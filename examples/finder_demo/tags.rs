use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

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

/// The tags this window shows, shared with each frame rather than copied into it.
#[derive(Clone, Default)]
pub(crate) struct Tags(Arc<HashMap<SharedString, Vec<usize>>>);

impl Tags {
    pub(crate) fn insert(&mut self, key: SharedString, tags: Vec<usize>) -> Option<Vec<usize>> {
        Arc::make_mut(&mut self.0).insert(key, tags)
    }

    pub(crate) fn remove(&mut self, key: &SharedString) -> Option<Vec<usize>> {
        Arc::make_mut(&mut self.0).remove(key)
    }

    pub(crate) fn entry(&mut self, key: SharedString) -> Entry<'_, SharedString, Vec<usize>> {
        Arc::make_mut(&mut self.0).entry(key)
    }
}

impl Deref for Tags {
    type Target = HashMap<SharedString, Vec<usize>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Extend<(SharedString, Vec<usize>)> for Tags {
    fn extend<I: IntoIterator<Item = (SharedString, Vec<usize>)>>(&mut self, marks: I) {
        Arc::make_mut(&mut self.0).extend(marks);
    }
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

pub(crate) fn finder_edge(color: u8) -> Hsla {
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

/// Tags as the disk has them; background work only.
pub(crate) fn read_row_tags(path: &str) -> Vec<usize> {
    finder_fs::tags::of(Path::new(path))
        .iter()
        .filter_map(|tag| tag_index(&tag.name))
        .collect()
}

/// Each path's tags once read, by folder; `None` while its read is queued.
#[derive(Default)]
struct Known {
    dirs: HashMap<PathBuf, HashMap<String, Option<Vec<usize>>>>,
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
    fn get(&self, path: &str) -> Option<&Option<Vec<usize>>> {
        let (dir, name) = split(path);
        self.dirs.get(dir)?.get(name)
    }

    fn insert(&mut self, path: String, tags: Option<Vec<usize>>) {
        let (dir, name) = split(&path);
        let names = match self.dirs.entry(dir.to_path_buf()) {
            Entry::Occupied(names) => names.into_mut(),
            Entry::Vacant(room) => {
                self.order.push_back(dir.to_path_buf());
                room.insert(HashMap::new())
            }
        };
        if names.insert(name.to_owned(), tags).is_none() {
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

    fn forget(&mut self, dir: &Path) {
        if let Some(names) = self.dirs.remove(dir) {
            self.held -= names.len();
            self.order.retain(|held| held != dir);
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
            KNOWN.lock().unwrap().insert(path, Some(tags));
            LANDED.fetch_add(1, Ordering::Release);
            crate::read::EVENTS.ring();
        }
    });
    Mutex::new(send)
});

/// Menus ask for a row's tags as they are built; reading them there would stall the window on
/// a share that stopped answering, so an unread row reads as untagged until its read lands.
pub(crate) fn row_tags(path: &str) -> Vec<usize> {
    let mut known = KNOWN.lock().unwrap();
    if let Some(tags) = known.get(path) {
        return tags.clone().unwrap_or_default();
    }
    known.insert(path.to_owned(), None);
    let _ = READER.lock().unwrap().send(path.to_owned());
    Vec::new()
}

/// How many reads have landed.
pub(crate) fn tags_landed() -> u64 {
    LANDED.load(Ordering::Acquire)
}

/// A folder changed on disk: its items' tags are read again when next asked for.
pub(crate) fn forget_tags_in(dir: &Path) {
    KNOWN.lock().unwrap().forget(dir);
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
        cx.spawn(async move |this, cx| {
            let carried = cx
                .background_executor()
                .spawn(async move {
                    write(&paths);
                    paths
                        .iter()
                        .map(|path| {
                            let key = path.to_string_lossy().into_owned();
                            let tags = read_row_tags(&key);
                            KNOWN
                                .lock()
                                .unwrap()
                                .insert(key.clone(), Some(tags.clone()));
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
