//! A sidebar is `NSSplitViewItem.sidebar` — a property of the *window's* view hierarchy, not of a
//! rectangle inside it. So it cannot be shown in this window's content pane, which is itself the
//! non-sidebar pane of a split view. Every control here drives a real sidebar in a second window.
//!
//! Most of [`SidebarOptions`] is honoured on install and then live: `set_row_size`,
//! `set_thickness`, `set_thickness_limits`, `set_toolbar`, `set_collapsed`, `set_expand_groups`
//! and `set_backdrop` all reach into the installed shell. [`PaneAlign`] is the exception — it
//! decides how GPUI's view is framed at install — so changing it reopens the window.
//!
//! The popped-out window never writes back into [`Demo`]. It is opened from inside a `Demo`
//! listener, gpui draws it before that listener returns, and a cross-entity update from its
//! `render` aborts the process with gpui's double-lease panic. This page reads the window instead.

use crate::ui::*;
use crate::{Demo, popout};
use gpui::{AnyElement, Context, Render, SharedString, Window, WindowHandle, div, prelude::*, px};
use native_gpui::sidebar::{
    NativeSidebar, PaneAlign, SidebarItem, SidebarOptions, SidebarProbe, SidebarRowSize,
    sidebar_group, sidebar_item,
};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{
    Material, TrackingMode, native_popup, native_root, native_segmented, native_slider,
    native_switch,
};

const ROW_SIZES: &[(&str, SidebarRowSize)] = &[
    ("System", SidebarRowSize::System),
    ("Small", SidebarRowSize::Small),
    ("Medium", SidebarRowSize::Medium),
    ("Large", SidebarRowSize::Large),
];

const BACKDROPS: &[(&str, Option<Material>)] = &[
    ("WindowBackground", Some(Material::WindowBackground)),
    ("Sidebar", Some(Material::Sidebar)),
    ("Titlebar", Some(Material::Titlebar)),
    ("Header", Some(Material::Header)),
    ("Popover", Some(Material::Popover)),
    ("Menu", Some(Material::Menu)),
    ("Hud", Some(Material::Hud)),
    ("Selection", Some(Material::Selection)),
    ("Card", Some(Material::Card)),
    ("none", None),
];

const ALIGNS: &[(&str, PaneAlign)] = &[
    ("WindowAligned", PaneAlign::WindowAligned),
    ("FillPane (broken control)", PaneAlign::FillPane),
];

pub struct State {
    pub window: Option<WindowHandle<SidebarWindow>>,
    pub thickness: f64,
    pub min: f64,
    pub max: f64,
    pub toolbar: bool,
    pub collapsed: bool,
    pub expand_groups: bool,
    pub reorderable: bool,
    pub row_size: usize,
    pub backdrop: usize,
    pub align: usize,
}

impl Default for State {
    fn default() -> Self {
        let defaults = SidebarOptions::default();
        Self {
            window: None,
            thickness: defaults.thickness.to_f64(),
            min: defaults.min_thickness.to_f64(),
            max: defaults.max_thickness.to_f64(),
            toolbar: defaults.toolbar,
            collapsed: defaults.collapsed,
            expand_groups: defaults.expand_groups,
            reorderable: defaults.reorderable,
            row_size: 0,
            backdrop: 0,
            align: 0,
        }
    }
}

impl State {
    fn options(&self) -> SidebarOptions {
        SidebarOptions {
            thickness: px(self.thickness as f32),
            min_thickness: px(self.min as f32),
            max_thickness: px(self.max as f32),
            toolbar: self.toolbar,
            collapsed: self.collapsed,
            expand_groups: self.expand_groups,
            reorderable: self.reorderable,
            row_size: ROW_SIZES[self.row_size].1,
            align: ALIGNS[self.align].1,
            adopt_below_layer: SidebarOptions::default().adopt_below_layer,
            backdrop: BACKDROPS[self.backdrop].1,
        }
    }
}

/// A Finder-shaped tree: groups, SF Symbols, a nested child and two count badges.
fn items() -> Vec<SidebarItem> {
    vec![
        sidebar_group("favorites", "Favorites").children([
            sidebar_item("airdrop", "AirDrop").symbol("dot.radiowaves.right"),
            sidebar_item("recents", "Recents").symbol("clock").badge(12),
            sidebar_item("applications", "Applications").symbol("folder"),
            sidebar_item("desktop", "Desktop").symbol("menubar.dock.rectangle"),
            sidebar_item("documents", "Documents")
                .symbol("doc")
                .child(sidebar_item("invoices", "Invoices").symbol("doc.text")),
            sidebar_item("downloads", "Downloads")
                .symbol("arrow.down.circle")
                .badge(3),
        ]),
        sidebar_group("locations", "Locations").children([
            sidebar_item("icloud", "iCloud Drive").symbol("icloud"),
            sidebar_item("network", "Network").symbol("globe"),
            sidebar_item("trash", "Trash").symbol("trash"),
        ]),
        sidebar_group("tags", "Tags").children([
            sidebar_item("red", "Red").symbol("circle.fill"),
            // A name no macOS knows: the glyph is absent and the label must still line up, which
            // is why the cell never hides its image view.
            sidebar_item("unknown", "Unknown symbol").symbol("not.a.real.sf.symbol"),
            // No symbol at all, so the cell has no image view to indent from and AppKit puts the
            // label at the cell's leading edge — visibly left of its lettered siblings.
            sidebar_item("untagged", "Untagged"),
        ]),
    ]
}

/// The popped-out window's root. It owns the real sidebar; this page only drives it.
pub struct SidebarWindow {
    sidebar: Option<NativeSidebar>,
    options: SidebarOptions,
    selected: SharedString,
}

impl SidebarWindow {
    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(window, cx, self.options.clone());
        sidebar.update(&items());
        sidebar.set_selected(Some(&self.selected));

        let entity = cx.entity().downgrade();
        sidebar.on_select(move |id, _, cx| {
            let Some(id) = id.clone() else { return };
            entity
                .update(cx, |this: &mut SidebarWindow, cx| {
                    this.selected = id;
                    cx.notify();
                })
                .ok();
        });
        self.sidebar = Some(sidebar);
    }

    /// Apply everything that can change without reinstalling. `collapsed` goes last: moving the
    /// divider expands the pane, so a thickness write would otherwise undo it.
    fn apply(&mut self, options: SidebarOptions) {
        let Some(sidebar) = &self.sidebar else { return };
        sidebar.set_row_size(options.row_size);
        sidebar.set_thickness_limits(options.min_thickness, options.max_thickness);
        sidebar.set_thickness(options.thickness);
        sidebar.set_toolbar(options.toolbar);
        sidebar.set_expand_groups(options.expand_groups);
        if let Some(material) = options.backdrop {
            sidebar.set_backdrop(material);
        }
        sidebar.set_collapsed(options.collapsed);
        self.options = options;
    }

    fn probe(&self) -> Option<SidebarProbe> {
        self.sidebar.as_ref().map(|s| s.probe())
    }

    /// The first row carrying an icon, so the reported label and icon offsets are an item row's
    /// rather than a group header's zeroes.
    fn sample_row(&self) -> Option<(f64, f64, f64, f64)> {
        let sidebar = self.sidebar.as_ref()?;
        (0..sidebar.probe().rows)
            .filter_map(|row| sidebar.row_layout(row))
            .find(|l| l.icon.size.width > 0.)
            .map(|l| {
                (
                    l.row.size.height,
                    l.cell.origin.x,
                    l.icon.size.height,
                    l.title.origin.x,
                )
            })
    }
}

impl Render for SidebarWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install(window, cx);
        }

        let metrics = self
            .sidebar
            .as_ref()
            .map(|s| s.metrics())
            .unwrap_or_default();
        let probe = self.probe();
        let sample = self.sample_row();

        let line = |text: String| {
            div()
                .text_sm()
                .text_color(system_color(SystemColor::SecondaryLabel))
                .child(SharedString::from(text))
                .into_any_element()
        };

        native_root(
            div().relative().size_full().child(
                div()
                    .absolute()
                    .left(metrics.left)
                    .top(metrics.top)
                    .right_0()
                    .bottom_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p(px(28.))
                    .child(
                        div()
                            .text_2xl()
                            .text_color(system_color(SystemColor::Label))
                            .child(SharedString::from(self.selected.to_string())),
                    )
                    .children(probe.map(|p| {
                        line(format!(
                            "rowHeight {:.1} · rowSizeStyle {} → effective {} · indentPerLevel {:.1}",
                            p.row_height,
                            p.row_size_style,
                            p.effective_row_size_style,
                            p.indentation_per_level
                        ))
                    }))
                    .children(sample.map(|(row, cell, icon, title)| {
                        line(format!(
                            "row h {row:.1} · cell x {cell:.1} · icon {icon:.1}pt · label x {title:.1}"
                        ))
                    }))
                    .child(line(format!(
                        "gutter left {:.0} top {:.0} · thickness {:.0}{}",
                        metrics.left.to_f64(),
                        metrics.top.to_f64(),
                        metrics.thickness.to_f64(),
                        if metrics.collapsed { " · collapsed" } else { "" }
                    )))
                    .children(probe.map(|p| {
                        line(format!(
                            "backdrop NSVisualEffectMaterial {} · toolbar items {} · rows {}",
                            p.backdrop_material, p.toolbar_items, p.rows
                        ))
                    }))
                    .child(line(
                        "Every row, its font, its icon box and the selection pill are AppKit's."
                            .into(),
                    )),
            ),
        )
    }
}

/// Open the second window, or bring the existing one forward.
///
/// Deferred: `open_window` draws the new window before returning, and this runs inside a `Demo`
/// listener that still holds `Demo`'s lease.
fn open(window: &mut Window, cx: &mut Context<Demo>) {
    cx.defer_in(window, |d: &mut Demo, _, cx| {
        if let Some(handle) = d.sidebar_page.window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            d.sidebar_page.window = None;
        }
        let options = d.sidebar_page.options();
        d.sidebar_page.window = popout(cx, (240., 200.), (760., 560.), move |_, _| SidebarWindow {
            sidebar: None,
            options,
            selected: "recents".into(),
        });
        cx.notify();
    });
}

/// Push the current options at the open window. Reinstall-only fields go through [`reopen`].
fn push(d: &mut Demo, cx: &mut Context<Demo>) {
    let Some(handle) = d.sidebar_page.window else {
        return;
    };
    let options = d.sidebar_page.options();
    if handle
        .update(cx, |w: &mut SidebarWindow, _, cx| {
            w.apply(options);
            cx.notify();
        })
        .is_err()
    {
        d.sidebar_page.window = None;
    }
}

/// `PaneAlign` is read once, during install, so the only way to change it is a fresh window.
fn reopen(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) {
    if let Some(handle) = d.sidebar_page.window.take() {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
    open(window, cx);
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let (thickness, min, max) = (
        d.sidebar_page.thickness,
        d.sidebar_page.min,
        d.sidebar_page.max,
    );
    let (toolbar, collapsed, expand_groups, reorderable) = (
        d.sidebar_page.toolbar,
        d.sidebar_page.collapsed,
        d.sidebar_page.expand_groups,
        d.sidebar_page.reorderable,
    );
    let (row_size, backdrop, align) = (
        d.sidebar_page.row_size,
        d.sidebar_page.backdrop,
        d.sidebar_page.align,
    );

    // Read the popped-out window rather than having it write here — see the module note.
    let live = d.sidebar_page.window.and_then(|handle| {
        handle
            .read(cx)
            .ok()
            .map(|w| (w.probe(), w.sample_row(), w.selected.clone()))
    });
    let open_label = if live.is_some() {
        "Bring the sidebar window forward"
    } else {
        "Open a window with a real sidebar"
    };
    let probe = live.as_ref().and_then(|(p, _, _)| *p);
    let sample = live.as_ref().and_then(|(_, s, _)| *s);
    let selected = live.map(|(_, _, id)| id);

    stack(vec![
        note(
            "A sidebar configures the window, so it is demonstrated in a second window. \
             Open it, then every control below moves it live.",
        ),
        demo(
            "the window",
            native_segmented("open", 0)
                .segments([open_label])
                .tracking_mode(TrackingMode::Momentary)
                .on_change(cx.listener(|_: &mut Demo, _: &usize, window, cx| {
                    open(window, cx);
                }))
                .into_any_element(),
        ),
        section(
            "rows",
            vec![
                knob(
                    "row_size",
                    ROW_SIZES[row_size].0,
                    native_popup("row_size", row_size)
                        .items(ROW_SIZES.iter().map(|(name, _)| *name))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.sidebar_page.row_size = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "NSTableView.rowSizeStyle. Small 24pt rows / 11pt labels, Medium 32 / 13, \
                     Large 40 / 15. System asks macOS, which answers Medium on 26 — Finder's \
                     metrics exactly. Leaving the property alone gives AppKit's own Custom \
                     default: .sourceList's 24pt row around a 13pt label, which is not a size \
                     macOS ships.",
                ),
                caption(match (probe, sample) {
                    (Some(p), Some((row, cell, icon, title))) => format!(
                        "live: rowHeight {:.1} · style {} → {} · row h {row:.1} · cell x {cell:.1} \
                         · icon {icon:.1}pt · label x {title:.1}",
                        p.row_height, p.row_size_style, p.effective_row_size_style
                    ),
                    (Some(p), None) => format!(
                        "live: rowHeight {:.1} · style {} → {}",
                        p.row_height, p.row_size_style, p.effective_row_size_style
                    ),
                    _ => "live: open the window to read AppKit back".into(),
                }),
                caption(
                    "Finder, measured through the same accessibility API: item row 32.0, group \
                     header 19.0, 13.0 above a group, cell inset 10.0, label 45.0 from the row's \
                     left edge. System matches all five.",
                ),
            ],
        ),
        section(
            "thickness",
            vec![
                knob(
                    "thickness",
                    format!("{thickness:.0}pt"),
                    native_slider("thickness", thickness)
                        .range(min, max)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.sidebar_page.thickness = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "min_thickness",
                    format!("{min:.0}pt"),
                    native_slider("min", min)
                        .range(120., 300.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.sidebar_page.min = *value;
                            d.sidebar_page.thickness = d.sidebar_page.thickness.max(*value);
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "max_thickness",
                    format!("{max:.0}pt"),
                    native_slider("max", max)
                        .range(240., 500.)
                        .on_change(cx.listener(|d: &mut Demo, value: &f64, _, cx| {
                            d.sidebar_page.max = *value;
                            d.sidebar_page.thickness = d.sidebar_page.thickness.min(*value);
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "AppKit clamps the divider to the item's own min and max, so dragging the \
                     limits past the current thickness moves it.",
                ),
            ],
        ),
        section(
            "shell",
            vec![
                knob(
                    "toolbar",
                    toolbar.to_string(),
                    native_switch("toolbar", toolbar)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.sidebar_page.toolbar = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "collapsed",
                    collapsed.to_string(),
                    native_switch("collapsed", collapsed)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.sidebar_page.collapsed = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "expand_groups",
                    expand_groups.to_string(),
                    native_switch("expand_groups", expand_groups)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.sidebar_page.expand_groups = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "reorderable",
                    reorderable.to_string(),
                    native_switch("reorderable", reorderable)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, _, cx| {
                            d.sidebar_page.reorderable = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "backdrop",
                    BACKDROPS[backdrop].0,
                    native_popup("backdrop", backdrop)
                        .items(BACKDROPS.iter().map(|(name, _)| *name))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                            d.sidebar_page.backdrop = *value;
                            push(d, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                knob(
                    "align",
                    ALIGNS[align].0,
                    native_popup("align", align)
                        .items(ALIGNS.iter().map(|(name, _)| *name))
                        .on_change(cx.listener(|d: &mut Demo, value: &usize, window, cx| {
                            d.sidebar_page.align = *value;
                            reopen(d, window, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "Turning the toolbar off drops the pane's top safe-area inset, which is where \
                     SidebarMetrics.top comes from instead of a constant. `align` is read once at \
                     install, so changing it reopens the window; FillPane is the deliberately \
                     broken control arm.",
                ),
                caption(
                    "`backdrop: none` leaves the window unbacked and cannot be applied live — \
                     there is no view to retint — so pick it before opening the window.",
                ),
            ],
        ),
        props(&[
            ("SidebarOptions.row_size", ROW_SIZES[row_size].0.to_string()),
            (
                "probe().row_height",
                probe.map_or("—".into(), |p| format!("{:.1}", p.row_height)),
            ),
            (
                "probe().effective_row_size_style",
                probe.map_or("—".into(), |p| {
                    format!("{} (2 = Medium)", p.effective_row_size_style)
                }),
            ),
            (
                "probe().effective_style",
                probe.map_or("—".into(), |p| {
                    format!("{} (3 = SourceList)", p.effective_style)
                }),
            ),
            (
                "probe().rows",
                probe.map_or("—".into(), |p| p.rows.to_string()),
            ),
            (
                "probe().toolbar_items",
                probe.map_or("—".into(), |p| p.toolbar_items.to_string()),
            ),
            (
                "probe().backdrop_material",
                probe.map_or("—".into(), |p| p.backdrop_material.to_string()),
            ),
            (
                "on_select",
                selected.map_or("—".into(), |id| id.to_string()),
            ),
            ("set_row_size / set_thickness", "live".into()),
            ("set_toolbar / set_collapsed", "live".into()),
            ("set_expand_groups / set_backdrop", "live".into()),
            (
                "reorderable",
                "install-only; reopen the window after changing it".into(),
            ),
            (
                "adopt_below_layer",
                "install-only; needs a Layer::Below host to show anything".into(),
            ),
            (
                "BADGE_GAP",
                format!("{}pt", native_gpui::sidebar::BADGE_GAP),
            ),
        ]),
    ])
}
