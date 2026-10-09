use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use convert::{Format, Intent, Kind, Probe, Report, ffmpeg_available, probe, relevant_targets};
use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, MouseButton, Render, SharedString, TextAlign,
    Window, div, hsla, img, prelude::*, px, relative, uniform_list,
};
use native_gpui::tokens::{FileType, SystemColor, file_icon, system_color};
use native_gpui::{
    ButtonBezel, CheckState, ControlSize, NativeButton, NativePopup, glass, native_button,
    native_checkbox, native_popup, native_progress, native_root, native_segmented,
    set_corner_radius, set_window_flags, window_flags,
};

use crate::{drop, jobs, open, thumbs};

const GRID_MIN_COL: f32 = 148.;
const GRID_GAP: f32 = 24.;
const GRID_ROW_GAP: f32 = 28.;
const GRID_PAD: f32 = 32.;
const GRID_LABEL: f32 = 48.;
const LIST_ICON: f32 = 48.;
const LIST_ROW: f32 = 72.;
const POSTER_RADIUS: f32 = 8.;
const STACK_W: f32 = 140.;
const STACK_H: f32 = 92.;
const RADIUS: f32 = 26.;
const CHROME: f32 = 36.;
const CHROME_INSET: f32 = 10.;

#[derive(Copy, Clone, PartialEq, Eq)]
enum Scene {
    Well,
    Browse,
}

#[derive(Default)]
pub struct Item {
    pub path: PathBuf,
    pub probe: Option<Probe>,
    pub dest: Option<PathBuf>,
    pub report: Option<Report>,
    pub error: Option<SharedString>,
    pub is_busy: bool,
}

impl Item {
    fn revealed(&self) -> &Path {
        self.dest
            .as_deref()
            .filter(|dest| dest.exists())
            .unwrap_or(&self.path)
    }
}

pub struct ConvertDemo {
    pub items: Vec<Item>,
    scene: Scene,
    is_grid: bool,
    pub target: Format,
    pub intent: Intent,
    pub should_keep_metadata: bool,
    pub is_busy: bool,
    pub done: usize,
    pub total: usize,
    pub notice: Option<SharedString>,
    pub summary: Option<SharedString>,
    pub output_dir: Option<PathBuf>,
    pub cancel: Arc<AtomicBool>,
    hover: Option<usize>,
    is_window_styled: bool,
}

impl ConvertDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this
                    .update(cx, |_, cx| {
                        if thumbs::has_pending() || drop::has_dropped() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            items: Vec::new(),
            scene: Scene::Well,
            is_grid: true,
            target: Format::Webp,
            intent: Intent::High,
            should_keep_metadata: true,
            is_busy: false,
            done: 0,
            total: 0,
            notice: None,
            summary: None,
            output_dir: None,
            cancel: Arc::new(AtomicBool::new(false)),
            hover: None,
            is_window_styled: false,
        }
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let mut folders = 0;
        let mut unknown = 0;
        let mut added = Vec::new();
        for path in paths {
            if path.is_dir() {
                folders += 1;
                continue;
            }
            if !path.is_file() || self.items.iter().any(|i| i.path == path) {
                continue;
            }
            if Format::from_path(&path).is_none() {
                unknown += 1;
                continue;
            }
            self.items.push(Item {
                path: path.clone(),
                ..Item::default()
            });
            added.push(path);
        }
        for path in added.iter().take(4) {
            thumbs::prefetch(path);
        }
        self.sync_target();
        self.notice = notice_for(folders, unknown, &self.items, &[]);
        self.summary = None;
        if added.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    added
                        .into_iter()
                        .map(|path| (probe(&path), path))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |demo, cx| {
                let mut notes = Vec::new();
                for (result, path) in results {
                    match result {
                        Ok(probed) => {
                            if let Some(item) = demo.items.iter_mut().find(|i| i.path == path) {
                                item.probe = Some(probed);
                            }
                        }
                        Err(err) => {
                            demo.items.retain(|i| i.path != path);
                            notes.push(err.to_string());
                        }
                    }
                }
                demo.sync_target();
                if !notes.is_empty() || demo.notice.is_none() {
                    demo.notice = notice_for(0, notes.len(), &demo.items, &notes);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn first_format(&self) -> Option<Format> {
        self.items.iter().find_map(|i| {
            i.probe
                .as_ref()
                .map(|p| p.format)
                .or_else(|| Format::from_path(&i.path))
        })
    }

    fn sync_target(&mut self) {
        let Some(first) = self.first_format() else {
            return;
        };
        if self.target.kind() != first.kind() {
            self.target = match first.kind() {
                Kind::Image => Format::Webp,
                Kind::Audio => Format::M4a,
                Kind::Font => Format::Woff2,
            };
        }
    }

    fn total_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter_map(|i| i.probe.as_ref().map(|p| p.bytes))
            .sum()
    }

    fn targets(&self) -> &'static [Format] {
        relevant_targets(self.first_format().unwrap_or(Format::Png))
    }

    fn target_index(&self) -> usize {
        self.targets()
            .iter()
            .position(|f| *f == self.target)
            .unwrap_or(0)
    }

    fn ready_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| can_convert(i, self.target))
            .count()
    }

    fn dest_label(&self) -> String {
        self.output_dir
            .as_deref()
            .map_or_else(|| "Next to originals".into(), folder_name)
    }

    fn converting(&self) -> String {
        format!(
            "Converting {} of {}…",
            self.done.min(self.total),
            self.total
        )
    }

    fn show(&mut self, scene: Scene, cx: &mut Context<Self>) {
        self.scene = scene;
        cx.notify();
    }

    fn advance(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            Self::choose_files(cx);
        } else {
            self.show(Scene::Browse, cx);
        }
    }

    fn clear(&mut self) {
        if self.is_busy {
            return;
        }
        self.items.clear();
        self.notice = None;
        self.summary = None;
        self.done = 0;
        self.total = 0;
        self.hover = None;
        self.scene = Scene::Well;
    }

    fn choose_files(cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let mut app = cx.to_async();
        open::pick_files(move |paths| {
            this.update(&mut app, |demo, cx| {
                demo.add_paths(paths, cx);
                cx.notify();
            })
            .ok();
        });
    }

    fn choose_folder(cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let mut app = cx.to_async();
        open::pick_folder(move |dir| {
            let Some(dir) = dir else {
                return;
            };
            this.update(&mut app, |demo, cx| {
                demo.output_dir = Some(dir);
                cx.notify();
            })
            .ok();
        });
    }

    pub fn finish_summary(&mut self) {
        let ok = self.items.iter().filter(|i| i.report.is_some()).count();
        let failed = self.items.iter().filter(|i| i.error.is_some()).count();
        if ok == 0 && failed == 0 {
            self.summary = None;
            return;
        }
        let saved: i64 = self
            .items
            .iter()
            .filter_map(|i| i.report.as_ref())
            .map(|r| r.bytes_in as i64 - r.bytes_out as i64)
            .sum();
        let mut parts = Vec::new();
        if ok > 0 {
            let files = count_noun(ok, "file", "files");
            parts.push(match &self.output_dir {
                Some(dir) => format!("Saved {files} in {}", folder_name(dir)),
                None => format!("Saved {files} next to the originals"),
            });
        }
        parts.extend(size_change(saved));
        if failed > 0 {
            parts.push(format!("{failed} didn’t convert"));
        }
        self.summary = Some(parts.join(" · ").into());
    }
}

fn notice_for(
    folders: usize,
    unknown: usize,
    items: &[Item],
    probe_notes: &[String],
) -> Option<SharedString> {
    if folders > 0 {
        return Some("Drop the files inside the folder, not the folder itself".into());
    }
    if let Some(note) = probe_notes.iter().find(|n| n.contains("font collections")) {
        return Some(note.clone().into());
    }
    if unknown > 0 {
        let verb = if unknown == 1 { "was" } else { "were" };
        return Some(format!("{unknown} {verb}n’t a photo, audio, or font file").into());
    }
    let has_audio = items
        .iter()
        .any(|i| i.probe.as_ref().is_some_and(|p| p.kind == Kind::Audio));
    (has_audio && !ffmpeg_available()).then(|| "Audio needs ffmpeg on this Mac".into())
}

pub fn count_noun(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn file_name(path: &Path) -> SharedString {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into()
}

fn caption(text: impl IntoElement, color: Hsla) -> Div {
    div().text_size(px(11.)).text_color(color).child(text)
}

impl Render for ConvertDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drop::prepare(window);
        if !self.is_window_styled {
            window.set_window_title("Convert");
            let mut flags = window_flags(window);
            flags.movable_by_background = true;
            flags.titled = false;
            set_window_flags(window, flags);
            set_corner_radius(window, Some(px(RADIUS)));
            self.is_window_styled = true;
        }
        self.add_paths(drop::take_dropped(), cx);
        thumbs::collect();

        let secondary = system_color(SystemColor::SecondaryLabel);
        native_root(
            div()
                .relative()
                .size_full()
                .text_color(system_color(SystemColor::Label))
                .child(
                    div().absolute().size_full().child(
                        glass("convert.bg")
                            .frosting_desktop()
                            .corner_radius(px(RADIUS)),
                    ),
                )
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .on_action(cx.listener(|_, _: &crate::OpenFiles, _, cx| {
                            ConvertDemo::choose_files(cx);
                        }))
                        .on_action(cx.listener(|this, _: &crate::ConvertNow, _, cx| {
                            this.scene = Scene::Browse;
                            jobs::start(this, cx, None);
                            cx.notify();
                        }))
                        .child(match self.scene {
                            Scene::Well => well(self, secondary, window, cx),
                            Scene::Browse => browse(self, secondary, window, cx),
                        }),
                ),
        )
    }
}

fn well(
    demo: &ConvertDemo,
    secondary: Hsla,
    window: &Window,
    cx: &mut Context<ConvertDemo>,
) -> AnyElement {
    let n = demo.items.len();
    let button = if n == 0 {
        native_button("well.choose", "Choose files")
    } else {
        native_button("well.choose", count_noun(n, "file", "files"))
            .trailing_symbol("chevron.right")
    };
    let centered = |text: SharedString, color| {
        div()
            .mb_2()
            .flex()
            .justify_center()
            .child(caption(text, color))
    };

    div()
        .relative()
        .size_full()
        .child(pin_chrome(close_corner("well.close", cx), false))
        .child(pin_chrome(
            corner("well.next", "chevron.right", "Continue")
                .enabled(n > 0)
                .on_click(cx.listener(|this, _, _, cx| this.show(Scene::Browse, cx))),
            true,
        ))
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(CHROME_INSET + CHROME + 12.))
                .px(px(16.))
                .pb(px(16.))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.advance(cx)),
                        )
                        .child(stack_art(demo, window.scale_factor(), secondary)),
                )
                .when_some(demo.notice.clone(), |d, note| {
                    d.child(centered(note, system_color(SystemColor::SystemOrange)))
                })
                .child(centered(well_caption(demo), secondary))
                .child(
                    button
                        .bezel_style(ButtonBezel::Glass)
                        .control_size(ControlSize::ExtraLarge)
                        .width(relative(1.))
                        .default_button(true)
                        .on_click(cx.listener(|this, _, _, cx| this.advance(cx))),
                ),
        )
        .into_any_element()
}

fn well_caption(demo: &ConvertDemo) -> SharedString {
    if demo.items.is_empty() {
        return "Photos, audio, or fonts · drop here or click".into();
    }
    match demo.total_bytes() {
        0 => SharedString::default(),
        bytes => format_bytes(bytes).into(),
    }
}

fn stack_art(demo: &ConvertDemo, scale: f32, secondary: Hsla) -> AnyElement {
    if demo.items.is_empty() {
        return div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Drop files here"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(secondary)
                    .child("or click to choose"),
            )
            .into_any_element();
    }
    div()
        .w(px(220.))
        .h(px(140.))
        .relative()
        .children(
            demo.items
                .iter()
                .take(4)
                .enumerate()
                .rev()
                .map(|(i, item)| {
                    div()
                        .absolute()
                        .left(px(40. + (i as f32 - 1.0) * 14.0))
                        .top(px(18. + i as f32 * 8.0))
                        .child(poster_thumb(&item.path, STACK_W, STACK_H, scale, true))
                }),
        )
        .into_any_element()
}

fn poster_thumb(path: &Path, width: f32, height: f32, scale: f32, is_stack: bool) -> AnyElement {
    let slot = div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    let image = if is_stack {
        thumbs::well(path, scale)
    } else {
        thumbs::poster(path, scale)
    };
    let slot = if let Some(image) = image {
        let fit = image.size(0);
        let (w, h) = (u32::from(fit.width) as f32, u32::from(fit.height) as f32);
        let zoom = (width / w.max(1.)).min(height / h.max(1.));
        slot.child(
            img(image)
                .w(px(w * zoom))
                .h(px(h * zoom))
                .rounded(px(POSTER_RADIUS))
                .border(px(0.5))
                .border_color(hsla(0., 0., 1., 0.12)),
        )
    } else {
        let side = height * if is_stack { 0.85 } else { 0.55 };
        slot.children(
            file_icon(FileType::Path(&path.to_string_lossy()), px(side)).map(|icon| icon.img()),
        )
    };
    slot.into_any_element()
}

fn grid_metrics(viewport_w: f32) -> (usize, f32, f32) {
    let inner = (viewport_w - GRID_PAD).max(GRID_MIN_COL);
    let cols = ((inner + GRID_GAP) / (GRID_MIN_COL + GRID_GAP))
        .floor()
        .clamp(2., 8.) as usize;
    let cell = (inner - GRID_GAP * (cols as f32 - 1.)).max(GRID_MIN_COL) / cols as f32;
    (cols, cell, cell * 0.72)
}

fn browse(
    demo: &ConvertDemo,
    secondary: Hsla,
    window: &Window,
    cx: &mut Context<ConvertDemo>,
) -> AnyElement {
    let is_busy = demo.is_busy;

    div()
        .relative()
        .size_full()
        .child(pin_chrome(
            corner("back", "chevron.left", "Back")
                .on_click(cx.listener(|this, _, _, cx| this.show(Scene::Well, cx))),
            false,
        ))
        .child(pin_chrome(
            close_corner("browse.close", cx).enabled(!is_busy),
            true,
        ))
        .child(
            div()
                .absolute()
                .top(px(CHROME_INSET))
                .right(px(CHROME_INSET + CHROME + 8.))
                .h(px(CHROME))
                .flex()
                .items_center()
                .child(
                    native_segmented("view", if demo.is_grid { 0 } else { 1 })
                        .segments(["Grid", "List"])
                        .symbols(["square.grid.2x2", "list.bullet"])
                        .control_size(ControlSize::Large)
                        .enabled(!is_busy)
                        .on_change(cx.listener(|this, index: &usize, _, cx| {
                            this.is_grid = *index == 0;
                            cx.notify();
                        })),
                ),
        )
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(CHROME_INSET + CHROME + 8.))
                .px(px(16.))
                .pb(px(16.))
                .child(browse_header(demo, secondary))
                .when(is_busy, |d| {
                    let fraction = if demo.total == 0 {
                        0.0
                    } else {
                        demo.done as f64 / demo.total as f64
                    };
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_1()
                            .child(native_progress("batch", fraction).width(relative(1.)))
                            .child(caption(demo.converting(), secondary)),
                    )
                })
                .when_some(demo.notice.clone(), |d, note| {
                    d.child(caption(note, system_color(SystemColor::SystemOrange)))
                })
                .when_some(demo.summary.clone(), |d, summary| {
                    d.child(caption(summary, secondary))
                })
                .child(if demo.is_grid {
                    grid_body(demo, secondary, window, cx)
                } else {
                    list_body(demo, secondary, cx)
                })
                .child(browse_footer(demo, secondary, cx)),
        )
        .into_any_element()
}

fn corner(id: &'static str, symbol: &'static str, title: &'static str) -> NativeButton {
    native_button(id, title)
        .symbol(symbol)
        .bezel_style(ButtonBezel::Glass)
        .control_size(ControlSize::Large)
        .square(CHROME)
}

fn close_corner(id: &'static str, cx: &mut Context<ConvertDemo>) -> NativeButton {
    corner(id, "xmark", "Close").on_click(cx.listener(|this, _, window, cx| {
        if this.items.is_empty() {
            window.remove_window();
        } else {
            this.clear();
            cx.notify();
        }
    }))
}

fn pin_chrome(button: NativeButton, is_right: bool) -> AnyElement {
    let pin = div().absolute().top(px(CHROME_INSET));
    let pin = if is_right {
        pin.right(px(CHROME_INSET))
    } else {
        pin.left(px(CHROME_INSET))
    };
    pin.child(button).into_any_element()
}

fn browse_header(demo: &ConvertDemo, secondary: Hsla) -> AnyElement {
    let subtitle: SharedString = if demo.is_busy {
        format!("Converting {} of {}", demo.done.min(demo.total), demo.total).into()
    } else if let Some(summary) = &demo.summary {
        summary.clone()
    } else {
        format_bytes(demo.total_bytes()).into()
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .pb_2()
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(count_noun(demo.items.len(), "file", "files")),
        )
        .child(caption(subtitle, secondary))
        .into_any_element()
}

fn browse_footer(demo: &ConvertDemo, secondary: Hsla, cx: &mut Context<ConvertDemo>) -> AnyElement {
    let targets = demo.targets();
    let intent_index = Intent::all()
        .iter()
        .position(|i| *i == demo.intent)
        .unwrap_or(0);
    let n = demo.ready_count();
    let is_busy = demo.is_busy;
    let can_start = n > 0 && !is_busy;
    let title = if is_busy {
        demo.converting()
    } else {
        let plural = if n == 1 { "" } else { "s" };
        format!("Save {n} file{plural} as {}", demo.target.label())
    };
    let popup = |control: NativePopup| {
        div().flex_1().child(
            control
                .bezel_style(ButtonBezel::Glass)
                .control_size(ControlSize::Large)
                .width(relative(1.))
                .enabled(!is_busy),
        )
    };

    div()
        .flex()
        .flex_col()
        .gap_2()
        .pt_3()
        .child(
            native_button("dest.pick", demo.dest_label())
                .trailing_symbol("chevron.up.chevron.down")
                .bezel_style(ButtonBezel::Glass)
                .control_size(ControlSize::Large)
                .width(relative(1.))
                .enabled(!is_busy)
                .on_click(cx.listener(|_, _, _, cx| ConvertDemo::choose_folder(cx))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(popup(
                    native_popup("format", demo.target_index())
                        .items(targets.iter().map(|format| format.label()))
                        .on_change(cx.listener(move |this, index: &usize, _, cx| {
                            if let Some(format) = targets.get(*index) {
                                this.target = *format;
                            }
                            cx.notify();
                        })),
                ))
                .child(popup(
                    native_popup("intent", intent_index)
                        .items(Intent::all().iter().map(|intent| intent.label()))
                        .on_change(cx.listener(|this, index: &usize, _, cx| {
                            if let Some(intent) = Intent::all().get(*index) {
                                this.intent = *intent;
                            }
                            cx.notify();
                        })),
                )),
        )
        .when(demo.target.kind() != Kind::Font, |d| {
            d.child(
                native_checkbox("meta", demo.should_keep_metadata)
                    .title("Keep extra info in the file")
                    .enabled(!is_busy)
                    .on_change(cx.listener(|this, state, _, cx| {
                        this.should_keep_metadata = matches!(state, CheckState::On);
                        cx.notify();
                    })),
            )
        })
        .child(caption(format_hint(demo), secondary))
        .child(
            native_button("convert", title)
                .bezel_style(ButtonBezel::Glass)
                .control_size(ControlSize::ExtraLarge)
                .width(relative(1.))
                .default_button(can_start)
                .enabled(can_start)
                .on_click(cx.listener(|this, _, _, cx| {
                    jobs::start(this, cx, None);
                    cx.notify();
                })),
        )
        .when(is_busy, |d| {
            d.child(
                native_button("cancel", "Stop")
                    .bezel_style(ButtonBezel::Glass)
                    .control_size(ControlSize::Large)
                    .width(relative(1.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cancel.store(true, Ordering::Relaxed);
                        cx.notify();
                    })),
            )
        })
        .into_any_element()
}

fn format_hint(demo: &ConvertDemo) -> SharedString {
    if let Some(advice) = demo.target.advice() {
        if demo.intent == Intent::Web && demo.target.is_desktop_font() {
            return format!("{advice} For a website, pick WOFF2.").into();
        }
        return advice.into();
    }
    match &demo.output_dir {
        Some(dir) => format!(
            "Saves in {} · click a thumbnail to show it",
            folder_name(dir)
        )
        .into(),
        None => "Saves next to the originals · click a thumbnail to show it".into(),
    }
}

fn can_convert(item: &Item, target: Format) -> bool {
    !item.is_busy
        && item.report.is_none()
        && item
            .probe
            .as_ref()
            .is_some_and(|probe| probe.kind == target.kind())
}

fn grid_body(
    demo: &ConvertDemo,
    secondary: Hsla,
    window: &Window,
    cx: &Context<ConvertDemo>,
) -> AnyElement {
    let (cols, cell, thumb_h) = grid_metrics(f32::from(window.viewport_size().width));
    let row_h = thumb_h + GRID_LABEL + GRID_ROW_GAP;
    uniform_list(
        "grid",
        demo.items.len().max(1).div_ceil(cols),
        cx.processor(move |demo, range: Range<usize>, window, cx| {
            let scale = window.scale_factor();
            range
                .map(|row| {
                    div()
                        .h(px(row_h))
                        .flex()
                        .gap(px(GRID_GAP))
                        .pb(px(GRID_ROW_GAP))
                        .children((row * cols..(row + 1) * cols).map(|index| {
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .when(index < demo.items.len(), |d| {
                                    d.child(grid_cell(
                                        demo, index, cell, thumb_h, scale, secondary, cx,
                                    ))
                                })
                        }))
                })
                .collect::<Vec<_>>()
        }),
    )
    .flex_1()
    .into_any_element()
}

fn list_body(demo: &ConvertDemo, secondary: Hsla, cx: &Context<ConvertDemo>) -> AnyElement {
    uniform_list(
        "list",
        demo.items.len(),
        cx.processor(move |demo, range: Range<usize>, window, cx| {
            let scale = window.scale_factor();
            range
                .map(|index| list_row(demo, index, scale, secondary, cx))
                .collect::<Vec<_>>()
        }),
    )
    .flex_1()
    .into_any_element()
}

fn watch_hover(
    index: usize,
) -> impl Fn(&mut ConvertDemo, &bool, &mut Window, &mut Context<ConvertDemo>) + 'static {
    move |demo, hovered, _, cx| {
        if *hovered {
            demo.hover = Some(index);
        } else if demo.hover == Some(index) {
            demo.hover = None;
        }
        cx.notify();
    }
}

fn grid_cell(
    demo: &ConvertDemo,
    index: usize,
    cell: f32,
    thumb_h: f32,
    scale: f32,
    secondary: Hsla,
    cx: &Context<ConvertDemo>,
) -> impl IntoElement {
    let item = &demo.items[index];
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .id(("grid.cell", index))
        .on_hover(cx.listener(watch_hover(index)))
        .child(thumb_with_save(demo, index, cell, thumb_h, scale, cx))
        .child(
            div()
                .w(px(cell))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_align(TextAlign::Center)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(file_name(&item.path)),
        )
        .children(meta_lines(demo, item, false, secondary, Some(cell)))
}

fn list_row(
    demo: &ConvertDemo,
    index: usize,
    scale: f32,
    secondary: Hsla,
    cx: &Context<ConvertDemo>,
) -> impl IntoElement {
    let item = &demo.items[index];
    div()
        .flex()
        .items_center()
        .gap_3()
        .h(px(LIST_ROW))
        .w_full()
        .id(("list.row", index))
        .on_hover(cx.listener(watch_hover(index)))
        .child(thumb_with_save(
            demo, index, LIST_ICON, LIST_ICON, scale, cx,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.))
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(file_name(&item.path)),
                )
                .children(meta_lines(demo, item, true, secondary, None)),
        )
}

fn thumb_with_save(
    demo: &ConvertDemo,
    index: usize,
    width: f32,
    height: f32,
    scale: f32,
    cx: &Context<ConvertDemo>,
) -> impl IntoElement {
    let item = &demo.items[index];
    let can_save = demo.hover == Some(index) && !demo.is_busy && can_convert(item, demo.target);
    div()
        .relative()
        .flex_none()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |demo, _, _, _| {
                if let Some(item) = demo.items.get(index) {
                    open::reveal(item.revealed());
                }
            }),
        )
        .child(poster_thumb(&item.path, width, height, scale, false))
        .when(can_save, |d| {
            let title = if item.error.is_some() {
                "Try again"
            } else {
                "Save"
            };
            d.child(
                div().absolute().top(px(6.)).right(px(6.)).child(
                    native_button(("item.save", index), title)
                        .bezel_style(ButtonBezel::Glass)
                        .control_size(ControlSize::Large)
                        .on_click(cx.listener(move |demo, _, _, cx| {
                            jobs::start(demo, cx, Some(index));
                            cx.notify();
                        })),
                ),
            )
        })
}

fn meta_lines(
    demo: &ConvertDemo,
    item: &Item,
    should_show_plan: bool,
    secondary: Hsla,
    width: Option<f32>,
) -> impl Iterator<Item = Div> {
    let (from, to, change) = item_copy(item, demo.target, demo.intent, should_show_plan);
    let line = move |text: SharedString, color| {
        let line = caption(text, color)
            .text_ellipsis()
            .whitespace_nowrap()
            .overflow_hidden();
        match width {
            Some(width) => line.w(px(width)).text_align(TextAlign::Center),
            None => line,
        }
    };
    let to = (!to.is_empty()).then(|| line(to, change));
    std::iter::once(line(from, secondary)).chain(to)
}

fn item_copy(
    item: &Item,
    target: Format,
    intent: Intent,
    should_show_plan: bool,
) -> (SharedString, SharedString, Hsla) {
    let label = system_color(SystemColor::Label);
    let orange = system_color(SystemColor::SystemOrange);
    if item.is_busy {
        return (
            item_before(item.probe.as_ref()),
            "Converting…".into(),
            label,
        );
    }
    if let Some(err) = &item.error {
        return (item_before(item.probe.as_ref()), err.clone(), orange);
    }
    if let Some(report) = &item.report {
        let from = format!(
            "{} → {}",
            format_bytes(report.bytes_in),
            format_bytes(report.bytes_out)
        );
        let saved = report.bytes_in as i64 - report.bytes_out as i64;
        let change = match saved.signum() {
            1 => system_color(SystemColor::SystemGreen),
            -1 => orange,
            _ => label,
        };
        let mut bits: Vec<String> = size_change(saved).into_iter().collect();
        if let (Some(w), Some(h)) = (report.width_out, report.height_out) {
            bits.push(format!("{w}×{h}"));
        }
        return (from.into(), bits.join(" · ").into(), change);
    }
    let Some(probe) = &item.probe else {
        return (SharedString::default(), SharedString::default(), label);
    };
    let before = item_before(Some(probe));
    if probe.kind != target.kind() {
        return (before, "Left out of this format".into(), orange);
    }
    let planned = match convert::plan(probe, target, intent) {
        Ok(planned) => planned,
        Err(err) => return (before, err.to_string().into(), orange),
    };
    if !should_show_plan {
        return (before, SharedString::default(), label);
    }
    let mut bits = vec![format!("→ {}", target.label())];
    if let (Some(w), Some(h)) = (probe.width, probe.height) {
        let (out_w, out_h) = scaled_edge(w, h, intent.default_max_edge());
        if (out_w, out_h) != (w, h) {
            bits.push(format!("{out_w}×{out_h}"));
        }
    }
    match planned.path {
        convert::PathTaken::Copy => bits.push("kept as-is".into()),
        convert::PathTaken::Remux => bits.push("repacked".into()),
        convert::PathTaken::Encode => {}
    }
    (before, bits.join(" · ").into(), label)
}

fn item_before(probe: Option<&Probe>) -> SharedString {
    let Some(probe) = probe else {
        return SharedString::default();
    };
    let size = format_bytes(probe.bytes);
    if let (Some(w), Some(h)) = (probe.width, probe.height) {
        format!("{size} · {w}×{h}").into()
    } else if let Some(duration) = probe.duration {
        format!("{size} · {duration:.1}s").into()
    } else if probe.kind == Kind::Font {
        format!("{size} · {}", probe.format.label()).into()
    } else {
        size.into()
    }
}

fn size_change(saved: i64) -> Option<String> {
    match saved.signum() {
        1 => Some(format!("{} smaller", format_bytes(saved as u64))),
        -1 => Some(format!("{} larger", format_bytes(saved.unsigned_abs()))),
        _ => None,
    }
}

fn scaled_edge(width: u32, height: u32, max_edge: Option<u32>) -> (u32, u32) {
    let long = width.max(height);
    match max_edge {
        Some(max) if long > max => {
            let scale = max as f64 / long as f64;
            (
                ((width as f64 * scale).round() as u32).max(1),
                ((height as f64 * scale).round() as u32).max(1),
            )
        }
        _ => (width, height),
    }
}

fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let n = n as f64;
    if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{n:.0} B")
    }
}
