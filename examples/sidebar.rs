use std::env;

use gpui::{
    App, Bounds, Context, Render, SharedString, Window, WindowBounds, WindowOptions, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::sidebar::{
    NativeSidebar, SidebarItem, SidebarMetrics, SidebarOptions, sidebar_group, sidebar_item,
};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{native_root, tokens, window_options};

const WIN_W: f32 = 1000.;
const WIN_H: f32 = 640.;

type Row = (&'static str, &'static str, &'static str, i64, u32);

const MAILBOXES: &[Row] = &[
    ("inbox", "Inbox", "tray.full", 12, 0x0a84ff),
    ("sent", "Sent", "paperplane", 0, 0x30d158),
    ("drafts", "Drafts", "doc.text", 3, 0xff9f0a),
];

const SMART: &[Row] = &[
    ("flagged", "Flagged", "flag", 4, 0xff453a),
    ("unread", "Unread", "envelope.badge", 37, 0xbf5af2),
    ("attachments", "Attachments", "paperclip", 0, 0x64d2ff),
];

const TAGS: &[Row] = &[
    ("work", "Work", "circle.fill", 0, 0xff6482),
    ("travel", "Travel", "airplane", 0, 0x5e5ce6),
    ("later", "Read Later", "bookmark", 0, 0xac8e68),
];

const GROUPS: &[(&str, &str, &[Row])] = &[
    ("g-mailboxes", "Mailboxes", MAILBOXES),
    ("g-smart", "Smart Mailboxes", SMART),
    ("g-tags", "Tags", TAGS),
];

fn items() -> Vec<SidebarItem> {
    GROUPS
        .iter()
        .map(|(id, title, rows)| {
            sidebar_group(*id, *title).children(rows.iter().map(
                |&(id, title, symbol, badge, _)| {
                    let item = sidebar_item(id, title).symbol(symbol);
                    if badge > 0 { item.badge(badge) } else { item }
                },
            ))
        })
        .collect()
}

fn find(id: &str) -> Option<(&'static str, &'static Row)> {
    GROUPS
        .iter()
        .find_map(|(_, group, rows)| rows.iter().find(|row| row.0 == id).map(|row| (*group, row)))
}

struct Demo {
    sidebar: Option<NativeSidebar>,
    selected: Option<SharedString>,
    frames: u64,
    quit_at: Option<u64>,
}

impl Demo {
    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(
            window,
            cx,
            SidebarOptions {
                collapsed: env::var("NG_COLLAPSED").is_ok(),
                ..SidebarOptions::default()
            },
        );
        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, _, cx| {
            this.selected = id.clone();
            cx.notify();
        }));
        sidebar.update(&items());
        sidebar.set_selected(Some("inbox"));
        self.selected = Some("inbox".into());
        self.sidebar = Some(sidebar);
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install(window, cx);
        }
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }

        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
        let left = f32::from(metrics.left);
        let top = f32::from(metrics.top);

        let (title, accent, detail) = match self.selected.as_deref().and_then(|id| find(id)) {
            Some((group, &(_, title, _, badge, accent))) if badge > 0 => {
                (title, accent, format!("{group} · {badge} unread"))
            }
            Some((group, &(_, title, _, _, accent))) => {
                (title, accent, format!("{group} · all caught up"))
            }
            None => (
                "Nothing selected",
                0x3a3a3c,
                "Pick a row in the sidebar.".to_string(),
            ),
        };

        // No background: a fill would hide the sidebar's material and double the pane's own.
        native_root(
            div().relative().size_full().child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(WIN_W - left))
                    .h(px(WIN_H - top))
                    .flex()
                    .flex_col()
                    .gap_5()
                    .p_8()
                    .child(
                        div()
                            .w(px(220.))
                            .h(px(72.))
                            .rounded(px(10.))
                            .bg(rgb(accent)),
                    )
                    .child(
                        div()
                            .text_size(px(28.))
                            .text_color(system_color(SystemColor::Label))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(system_color(SystemColor::SecondaryLabel))
                            .child(detail),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(system_color(SystemColor::TertiaryLabel))
                            .child(format!(
                                "content pane inset {left:.0}pt from the window edge"
                            )),
                    ),
            ),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(200.), px(160.)),
                    size: size(px(WIN_W), px(WIN_H)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Demo {
                    sidebar: None,
                    selected: None,
                    frames: 0,
                    quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
