use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use convert::{Format, Intent, Kind, Probe, Report, ffmpeg_available, probe, relevant_targets};
use gpui::{
    Bounds, Context, FontWeight, Hsla, MouseButton, Render, SharedString, TextAlign, WeakEntity,
    Window, WindowBounds, div, img, point, prelude::*, px, relative, size, uniform_list,
};
use native_gpui::tokens::{FileType, SystemColor, file_icon, system_color};
use native_gpui::{
    ButtonBezel, CheckState, ControlSize, glass, native_button, native_checkbox, native_popup,
    native_progress, native_root, native_segmented, set_corner_radius, set_window_flags,
    window_flags,
};

use crate::{drop, jobs, open, thumbs};

const W: f32 = 500.;
const H: f32 = 620.;
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

pub struct Item {
    pub path: PathBuf,
    pub probe: Option<Probe>,
    pub dest: Option<PathBuf>,
    pub report: Option<Report>,
    pub error: Option<String>,
    pub is_busy: bool,
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
    pub notice: Option<String>,
    pub summary: Option<String>,
    pub output_dir: Option<PathBuf>,
    pub cancel: Arc<AtomicBool>,
    hover: Option<usize>,
    is_drop_ready: bool,
}

impl ConvertDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
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
            is_drop_ready: false,
        }
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let mut folders = 0usize;
        let mut unknown = 0usize;
        let mut added = Vec::new();
        for path in paths {
            if path.is_dir() {
                folders += 1;
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if self.items.iter().any(|i| i.path == path) {
                continue;
            }
            if Format::from_path(&path).is_none() {
                unknown += 1;
                continue;
            }
            self.items.push(Item {
                path: path.clone(),
                probe: None,
                dest: None,
                report: None,
                error: None,
                is_busy: false,
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
                        .map(|path| {
                            let result = probe(&path);
                            (path, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |demo, cx| {
                let mut unknown = 0usize;
                let mut probe_notes = Vec::new();
                for (path, result) in results {
                    match result {
                        Ok(probed) => {
                            if let Some(item) = demo.items.iter_mut().find(|i| i.path == path) {
                                item.probe = Some(probed);
                            }
                        }
                        Err(err) => {
                            demo.items.retain(|i| i.path != path);
                            unknown += 1;
                            probe_notes.push(err.to_string());
                        }
                    }
                }
                demo.sync_target();
                if unknown > 0 || !probe_notes.is_empty() {
                    demo.notice = notice_for(0, unknown, &demo.items, &probe_notes);
                } else if demo.notice.is_none() {
                    demo.notice = notice_for(0, 0, &demo.items, &[]);
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
        let targets = relevant_targets(first);
        if !targets.contains(&self.target) || self.target.kind() != first.kind() {
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
        let Some(first) = self.first_format() else {
            return convert::relevant_targets(Format::Png);
        };
        relevant_targets(first)
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
        match &self.output_dir {
            Some(dir) => dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string()),
            None => "Next to originals".into(),
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
            this.update(&mut app, |demo, cx| {
                if let Some(dir) = dir {
                    demo.output_dir = Some(dir);
                    cx.notify();
                }
            })
            .ok();
        });
    }

    pub fn finish_summary(&mut self) {
        let ok = self.items.iter().filter(|i| i.report.is_some()).count();
        let failed = self.items.iter().filter(|i| i.error.is_some()).count();
        let saved: i64 = self
            .items
            .iter()
            .filter_map(|i| i.report.as_ref())
            .map(|r| r.bytes_in as i64 - r.bytes_out as i64)
            .sum();
        if ok == 0 && failed == 0 {
            self.summary = None;
            return;
        }
        let mut parts = Vec::new();
        if ok > 0 {
            parts.push(match &self.output_dir {
                Some(dir) => format!(
                    "Saved {} in {}",
                    count_noun(ok, "file", "files"),
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.display().to_string())
                ),
                None => format!(
                    "Saved {} next to the originals",
                    count_noun(ok, "file", "files")
                ),
            });
        }
        if saved > 0 {
            parts.push(format!("{} smaller", format_bytes(saved as u64)));
        } else if saved < 0 {
            parts.push(format!("{} larger", format_bytes((-saved) as u64)));
        }
        if failed > 0 {
            parts.push(format!("{} didn’t convert", failed));
        }
        self.summary = Some(parts.join(" · "));
    }
}

fn notice_for(
    folders: usize,
    unknown: usize,
    items: &[Item],
    probe_notes: &[String],
) -> Option<String> {
    if folders > 0 {
        return Some("Drop the files inside the folder, not the folder itself".into());
    }
    if let Some(note) = probe_notes.iter().find(|n| n.contains("font collections")) {
        return Some(note.clone());
    }
    if unknown > 0 {
        return Some(format!(
            "{} {}n’t a photo, audio, or font file",
            unknown,
            if unknown == 1 { "was" } else { "were" }
        ));
    }
    let audio = items
        .iter()
        .any(|i| i.probe.as_ref().is_some_and(|p| p.kind == Kind::Audio));
    if audio && !ffmpeg_available() {
        return Some("Audio needs ffmpeg on this Mac".into());
    }
    None
}

fn count_noun(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

impl Render for ConvertDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drop::install(window);
        drop::hide_traffic_lights(window);
        drop::enable_resize(window);
        window.set_window_title("Convert");
        if !self.is_drop_ready {
            let mut flags = window_flags(window);
            flags.movable_by_background = true;
            flags.titled = false;
            set_window_flags(window, flags);
            set_corner_radius(window, Some(px(RADIUS)));
            self.is_drop_ready = true;
        }
        let incoming = drop::take_dropped();
        if !incoming.is_empty() {
            self.add_paths(incoming, cx);
        }
        thumbs::collect();

        let label = system_color(SystemColor::Label);
        let secondary = system_color(SystemColor::SecondaryLabel);

        native_root(
            div()
                .relative()
                .size_full()
                .text_color(label)
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
    secondary: gpui::Hsla,
    window: &mut Window,
    cx: &mut Context<ConvertDemo>,
) -> gpui::AnyElement {
    let n = demo.items.len();
    let scale = window.scale_factor();
    let is_empty = n == 0;

    div()
        .relative()
        .size_full()
        .child(pin_chrome(
            corner("well.close", "xmark", "Close").on_click(cx.listener(|this, _, window, cx| {
                if this.items.is_empty() {
                    window.remove_window();
                } else {
                    this.clear();
                    cx.notify();
                }
            })),
            false,
        ))
        .child(pin_chrome(
            corner("well.next", "chevron.right", "Continue")
                .enabled(!is_empty)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.scene = Scene::Browse;
                    cx.notify();
                })),
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
                        .on_mouse_down(MouseButton::Left, {
                            let is_empty = is_empty;
                            cx.listener(move |this, _, _, cx| {
                                if is_empty {
                                    ConvertDemo::choose_files(cx);
                                } else {
                                    this.scene = Scene::Browse;
                                    cx.notify();
                                }
                            })
                        })
                        .child(stack_art(demo, scale, secondary)),
                )
                .when_some(demo.notice.clone(), |d, note| {
                    d.child(
                        div().mb_2().flex().justify_center().child(
                            div()
                                .text_size(px(11.))
                                .text_color(system_color(SystemColor::SystemOrange))
                                .child(note),
                        ),
                    )
                })
                .child(
                    div().mb_2().flex().justify_center().child(
                        div()
                            .text_size(px(11.))
                            .text_color(secondary)
                            .child(well_caption(demo)),
                    ),
                )
                .child({
                    let button = if is_empty {
                        native_button("well.choose", "Choose files")
                    } else {
                        native_button("well.choose", count_noun(n, "file", "files"))
                            .trailing_symbol("chevron.right")
                    };
                    button
                        .bezel_style(ButtonBezel::Glass)
                        .control_size(ControlSize::ExtraLarge)
                        .width(relative(1.))
                        .default_button(true)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if is_empty {
                                ConvertDemo::choose_files(cx);
                            } else {
                                this.scene = Scene::Browse;
                                cx.notify();
                            }
                        }))
                }),
        )
        .into_any_element()
}

fn well_caption(demo: &ConvertDemo) -> SharedString {
    if demo.items.is_empty() {
        return "Photos, audio, or fonts · drop here or click".into();
    }
    let bytes = demo.total_bytes();
    if bytes == 0 {
        return SharedString::default();
    }
    format_bytes(bytes).into()
}

fn stack_art(demo: &ConvertDemo, scale: f32, secondary: gpui::Hsla) -> gpui::AnyElement {
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
    let shown: Vec<_> = demo.items.iter().take(4).collect();
    div()
        .w(px(220.))
        .h(px(140.))
        .relative()
        .children(shown.iter().enumerate().rev().map(|(i, item)| {
            let dx = (i as f32 - 1.0) * 14.0;
            let dy = (i as f32) * 8.0;
            div()
                .absolute()
                .left(px(40. + dx))
                .top(px(18. + dy))
                .child(poster_thumb(&item.path, STACK_W, STACK_H, scale, true))
        }))
        .into_any_element()
}

fn poster_thumb(
    path: &std::path::Path,
    width: f32,
    height: f32,
    scale: f32,
    is_stack: bool,
) -> gpui::AnyElement {
    let mut slot = div()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    let image = if is_stack {
        thumbs::well(path, scale)
    } else {
        thumbs::get(path, scale)
    };
    if let Some(image) = image {
        let fit = image.size(0);
        let (w, h) = (u32::from(fit.width) as f32, u32::from(fit.height) as f32);
        let zoom = (width / w.max(1.)).min(height / h.max(1.));
        slot = slot.child(
            img(image)
                .w(px(w * zoom))
                .h(px(h * zoom))
                .rounded(px(POSTER_RADIUS))
                .border(px(0.5))
                .border_color(Hsla {
                    h: 0.,
                    s: 0.,
                    l: 1.,
                    a: 0.12,
                }),
        );
    } else {
        let tag = path.to_string_lossy();
        let side = if is_stack {
            height * 0.85
        } else {
            height * 0.55
        };
        if let Some(icon) = file_icon(FileType::Path(&tag), px(side)) {
            slot = slot.child(icon.img());
        }
    }
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
    demo: &mut ConvertDemo,
    secondary: gpui::Hsla,
    window: &mut Window,
    cx: &mut Context<ConvertDemo>,
) -> gpui::AnyElement {
    let scale = window.scale_factor();
    let is_grid = demo.is_grid;
    let is_busy = demo.is_busy;
    let fraction = if demo.total == 0 {
        0.0
    } else {
        demo.done as f64 / demo.total as f64
    };

    div()
        .relative()
        .size_full()
        .child(pin_chrome(
            corner("back", "chevron.left", "Back").on_click(cx.listener(|this, _, _, cx| {
                this.scene = Scene::Well;
                cx.notify();
            })),
            false,
        ))
        .child(pin_chrome(
            corner("browse.close", "xmark", "Close")
                .enabled(!is_busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.items.is_empty() {
                        window.remove_window();
                    } else {
                        this.clear();
                        cx.notify();
                    }
                })),
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
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_1()
                            .child(native_progress("batch", fraction).width(relative(1.)))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(secondary)
                                    .child(format!(
                                        "Converting {} of {}…",
                                        demo.done.min(demo.total),
                                        demo.total
                                    )),
                            ),
                    )
                })
                .when_some(demo.notice.clone(), |d, note| {
                    d.child(
                        div()
                            .text_size(px(11.))
                            .text_color(system_color(SystemColor::SystemOrange))
                            .child(note),
                    )
                })
                .when_some(demo.summary.clone(), |d, sum| {
                    d.child(div().text_size(px(11.)).text_color(secondary).child(sum))
                })
                .child(if is_grid {
                    grid_body(
                        demo,
                        scale,
                        f32::from(window.viewport_size().width),
                        secondary,
                        cx,
                    )
                } else {
                    list_body(demo, scale, secondary, cx)
                })
                .child(browse_footer(demo, secondary, cx)),
        )
        .into_any_element()
}

fn corner(
    id: &'static str,
    symbol: &'static str,
    title: &'static str,
) -> native_gpui::NativeButton {
    native_button(id, title)
        .symbol(symbol)
        .bezel_style(ButtonBezel::Glass)
        .control_size(ControlSize::Large)
        .square(CHROME)
}

fn pin_chrome(button: native_gpui::NativeButton, is_right: bool) -> gpui::AnyElement {
    let pin = div().absolute().top(px(CHROME_INSET));
    let pin = if is_right {
        pin.right(px(CHROME_INSET))
    } else {
        pin.left(px(CHROME_INSET))
    };
    pin.child(button).into_any_element()
}

fn browse_header(demo: &ConvertDemo, secondary: gpui::Hsla) -> gpui::AnyElement {
    let n = demo.items.len();
    let subtitle = if demo.is_busy {
        format!("Converting {} of {}", demo.done.min(demo.total), demo.total)
    } else if let Some(sum) = &demo.summary {
        sum.clone()
    } else {
        format_bytes(demo.total_bytes())
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
                .child(count_noun(n, "file", "files")),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(secondary)
                .child(subtitle),
        )
        .into_any_element()
}

fn browse_footer(
    demo: &ConvertDemo,
    secondary: gpui::Hsla,
    cx: &mut Context<ConvertDemo>,
) -> gpui::AnyElement {
    let targets = demo.targets();
    let labels: Vec<String> = targets.iter().map(|f| f.label().to_string()).collect();
    let intent_i = Intent::all()
        .iter()
        .position(|i| *i == demo.intent)
        .unwrap_or(0);
    let n = demo.ready_count();
    let is_busy = demo.is_busy;
    let button = if is_busy {
        format!(
            "Converting {} of {}…",
            demo.done.min(demo.total),
            demo.total
        )
    } else {
        if n == 1 {
            format!("Save 1 file as {}", demo.target.label())
        } else {
            format!("Save {n} files as {}", demo.target.label())
        }
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
                .on_click(cx.listener(|_, _, _, cx| {
                    ConvertDemo::choose_folder(cx);
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div().flex_1().child(
                        native_popup("format", demo.target_index())
                            .items(labels)
                            .bezel_style(ButtonBezel::Glass)
                            .control_size(ControlSize::Large)
                            .width(relative(1.))
                            .enabled(!is_busy)
                            .on_change(cx.listener({
                                let targets = targets.to_vec();
                                move |this, index: &usize, _, cx| {
                                    if let Some(fmt) = targets.get(*index) {
                                        this.target = *fmt;
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
                )
                .child(
                    div().flex_1().child(
                        native_popup("intent", intent_i)
                            .items(Intent::all().iter().map(|intent| intent.label()))
                            .bezel_style(ButtonBezel::Glass)
                            .control_size(ControlSize::Large)
                            .width(relative(1.))
                            .enabled(!is_busy)
                            .on_change(cx.listener(|this, index: &usize, _, cx| {
                                if let Some(intent) = Intent::all().get(*index) {
                                    this.intent = *intent;
                                }
                                cx.notify();
                            })),
                    ),
                ),
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
        .child(
            div()
                .text_size(px(11.))
                .text_color(secondary)
                .child(format_hint(demo)),
        )
        .child(
            native_button("convert", button)
                .bezel_style(ButtonBezel::Glass)
                .control_size(ControlSize::ExtraLarge)
                .width(relative(1.))
                .default_button(n > 0 && !is_busy)
                .enabled(n > 0 && !is_busy)
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
    let dest = match &demo.output_dir {
        Some(dir) => format!(
            "Saves in {}",
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.display().to_string())
        ),
        None => "Saves next to the originals".into(),
    };
    format!("{dest} · click a thumbnail to show it").into()
}

fn can_convert(item: &Item, target: Format) -> bool {
    if item.is_busy || item.report.is_some() {
        return false;
    }
    item.probe
        .as_ref()
        .is_some_and(|probe| probe.kind == target.kind())
}

fn grid_body(
    demo: &ConvertDemo,
    scale: f32,
    viewport_w: f32,
    secondary: gpui::Hsla,
    cx: &mut Context<ConvertDemo>,
) -> gpui::AnyElement {
    let this = cx.entity().downgrade();
    let items = item_views(demo, false);
    let hover = demo.hover;
    let (cols, cell, thumb_h) = grid_metrics(viewport_w);
    let n = items.len().max(1);
    let rows = n.div_ceil(cols);
    let row_h = thumb_h + GRID_LABEL + GRID_ROW_GAP;
    uniform_list("grid", rows, move |range, _, _| {
        range
            .map(|row| {
                div()
                    .h(px(row_h))
                    .flex()
                    .gap(px(GRID_GAP))
                    .pb(px(GRID_ROW_GAP))
                    .children((0..cols).map(|col| {
                        let i = row * cols + col;
                        match items.get(i) {
                            Some(item) => div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(grid_cell(
                                    item,
                                    cell,
                                    thumb_h,
                                    scale,
                                    secondary,
                                    hover == Some(item.index),
                                    this.clone(),
                                ))
                                .into_any_element(),
                            None => div().flex_1().min_w(px(0.)).into_any_element(),
                        }
                    }))
            })
            .collect()
    })
    .flex_1()
    .into_any_element()
}

fn list_body(
    demo: &ConvertDemo,
    scale: f32,
    secondary: gpui::Hsla,
    cx: &mut Context<ConvertDemo>,
) -> gpui::AnyElement {
    let this = cx.entity().downgrade();
    let items = item_views(demo, true);
    let hover = demo.hover;
    uniform_list("list", items.len(), move |range, _, _| {
        range
            .map(|i| {
                items
                    .get(i)
                    .map(|item| {
                        list_row(
                            item,
                            scale,
                            secondary,
                            hover == Some(item.index),
                            this.clone(),
                        )
                    })
                    .unwrap_or_else(|| div().into_any_element())
            })
            .collect()
    })
    .flex_1()
    .into_any_element()
}

fn watch_hover(
    this: WeakEntity<ConvertDemo>,
    index: usize,
) -> impl Fn(&bool, &mut Window, &mut gpui::App) + 'static {
    move |hovered, _, cx| {
        this.update(cx, |demo, cx| {
            if *hovered {
                demo.hover = Some(index);
            } else if demo.hover == Some(index) {
                demo.hover = None;
            }
            cx.notify();
        })
        .ok();
    }
}

fn grid_cell(
    item: &ItemView,
    cell: f32,
    thumb_h: f32,
    scale: f32,
    secondary: gpui::Hsla,
    hovered: bool,
    this: WeakEntity<ConvertDemo>,
) -> gpui::AnyElement {
    let reveal = item.reveal.clone();
    let index = item.index;
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .id(("grid.cell", index))
        .on_hover(watch_hover(this.clone(), index))
        .child(thumb_with_save(
            item, cell, thumb_h, scale, hovered, this, reveal,
        ))
        .child(
            div()
                .w(px(cell))
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_align(TextAlign::Center)
                .text_ellipsis()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(item.name.clone()),
        )
        .child(meta_line(item.from.clone(), secondary, true, Some(cell)))
        .when(!item.to.is_empty(), |d| {
            d.child(meta_line(item.to.clone(), item.change, true, Some(cell)))
        })
        .into_any_element()
}

fn list_row(
    item: &ItemView,
    scale: f32,
    secondary: gpui::Hsla,
    hovered: bool,
    this: WeakEntity<ConvertDemo>,
) -> gpui::AnyElement {
    let reveal = item.reveal.clone();
    let index = item.index;
    div()
        .flex()
        .items_center()
        .gap_3()
        .h(px(LIST_ROW))
        .w_full()
        .id(("list.row", index))
        .on_hover(watch_hover(this.clone(), index))
        .child(thumb_with_save(
            item, LIST_ICON, LIST_ICON, scale, hovered, this, reveal,
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
                        .child(item.name.clone()),
                )
                .child(meta_line(item.from.clone(), secondary, false, None))
                .when(!item.to.is_empty(), |d| {
                    d.child(meta_line(item.to.clone(), item.change, false, None))
                }),
        )
        .into_any_element()
}

fn thumb_with_save(
    item: &ItemView,
    width: f32,
    height: f32,
    scale: f32,
    hovered: bool,
    this: WeakEntity<ConvertDemo>,
    reveal: PathBuf,
) -> gpui::AnyElement {
    div()
        .relative()
        .flex_none()
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            open::reveal(&reveal);
        })
        .child(poster_thumb(&item.path, width, height, scale, false))
        .when(hovered && item.can_save, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(6.))
                    .right(px(6.))
                    .child(item_save(item, this)),
            )
        })
        .into_any_element()
}

fn meta_line(
    text: SharedString,
    color: gpui::Hsla,
    center: bool,
    width: Option<f32>,
) -> gpui::AnyElement {
    let mut line = div()
        .text_size(px(11.))
        .text_color(color)
        .text_ellipsis()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text);
    if let Some(width) = width {
        line = line.w(px(width));
    }
    if center {
        line = line.text_align(TextAlign::Center);
    }
    line.into_any_element()
}

fn item_save(item: &ItemView, this: WeakEntity<ConvertDemo>) -> native_gpui::NativeButton {
    let index = item.index;
    let title = if item.is_retry { "Try again" } else { "Save" };
    native_button(("item.save", index), title)
        .bezel_style(ButtonBezel::Glass)
        .control_size(ControlSize::Large)
        .on_click(move |_, _, cx| {
            this.update(cx, |demo, cx| {
                jobs::start(demo, cx, Some(index));
                cx.notify();
            })
            .ok();
        })
}

struct ItemView {
    index: usize,
    path: PathBuf,
    reveal: PathBuf,
    name: SharedString,
    from: SharedString,
    to: SharedString,
    change: gpui::Hsla,
    can_save: bool,
    is_retry: bool,
}

fn item_views(demo: &ConvertDemo, should_show_plan: bool) -> Vec<ItemView> {
    demo.items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let reveal = item
                .dest
                .as_ref()
                .filter(|path| path.exists())
                .cloned()
                .unwrap_or_else(|| item.path.clone());
            let (from, to, change) = item_copy(item, demo.target, demo.intent, should_show_plan);
            ItemView {
                index,
                path: item.path.clone(),
                reveal,
                name: item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                from: from.into(),
                to: to.into(),
                change,
                can_save: can_convert(item, demo.target) && !demo.is_busy,
                is_retry: item.error.is_some() && item.report.is_none(),
            }
        })
        .collect()
}

fn item_copy(
    item: &Item,
    target: Format,
    intent: Intent,
    should_show_plan: bool,
) -> (String, String, gpui::Hsla) {
    let label = system_color(SystemColor::Label);
    let orange = system_color(SystemColor::SystemOrange);
    let green = system_color(SystemColor::SystemGreen);
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
        let mut bits = Vec::new();
        if report.grew() {
            bits.push(format!(
                "{} larger",
                format_bytes(report.bytes_out.saturating_sub(report.bytes_in))
            ));
        } else if report.bytes_out < report.bytes_in {
            bits.push(format!(
                "{} smaller",
                format_bytes(report.bytes_in.saturating_sub(report.bytes_out))
            ));
        }
        if let (Some(w), Some(h)) = (report.width_out, report.height_out) {
            bits.push(format!("{w}×{h}"));
        }
        let change = if report.grew() {
            orange
        } else if report.bytes_out < report.bytes_in {
            green
        } else {
            label
        };
        return (from, bits.join(" · "), change);
    }
    let Some(probe) = &item.probe else {
        return (String::new(), String::new(), label);
    };
    if probe.kind != target.kind() {
        return (
            item_before(Some(probe)),
            "Left out of this format".into(),
            orange,
        );
    }
    match convert::plan(probe, target, intent) {
        Err(err) => (item_before(Some(probe)), err.to_string(), orange),
        Ok(planned) => {
            if !should_show_plan {
                return (item_before(Some(probe)), String::new(), label);
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
            (item_before(Some(probe)), bits.join(" · "), label)
        }
    }
}

fn item_before(probe: Option<&Probe>) -> String {
    let Some(probe) = probe else {
        return String::new();
    };
    let size = format_bytes(probe.bytes);
    if let (Some(w), Some(h)) = (probe.width, probe.height) {
        return format!("{size} · {w}×{h}");
    }
    if let Some(duration) = probe.duration {
        return format!("{size} · {duration:.1}s");
    }
    if probe.kind == Kind::Font {
        return format!("{size} · {}", probe.format.label());
    }
    size
}

fn scaled_edge(width: u32, height: u32, max_edge: Option<u32>) -> (u32, u32) {
    let Some(max) = max_edge else {
        return (width, height);
    };
    let long = width.max(height);
    if long <= max {
        return (width, height);
    }
    let scale = max as f64 / long as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
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

pub fn window_bounds() -> WindowBounds {
    WindowBounds::Windowed(Bounds {
        origin: point(px(240.), px(160.)),
        size: size(px(W), px(H)),
    })
}
