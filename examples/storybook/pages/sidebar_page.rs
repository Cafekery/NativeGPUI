//! A sidebar is a property of the window, so this page drives one in a second window.

use crate::ui::*;
use crate::{Demo, popout};
use gpui::{AnyElement, Context, Render, SharedString, Window, WindowHandle, div, prelude::*, px};
use native_gpui::sidebar::{
    NativeSidebar, PaneAlign, SidebarItem, SidebarMove, SidebarOptions, SidebarProbe,
    SidebarRowSize, sidebar_group, sidebar_item,
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
            backdrop: BACKDROPS
                .iter()
                .position(|(_, material)| *material == defaults.backdrop)
                .unwrap_or(0),
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
            content_min_thickness: SidebarOptions::default().content_min_thickness,
            backdrop: BACKDROPS[self.backdrop].1,
        }
    }
}

fn items() -> Vec<SidebarItem> {
    vec![
        sidebar_group("favorites", "Favorites").children([
            sidebar_item("recents", "Recents").symbol("clock").badge(12),
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
            sidebar_item("volume", "Bakery")
                .symbol("externaldrive")
                .ejectable(true),
            sidebar_item("trash", "Trash").symbol("trash"),
        ]),
        sidebar_group("glyphs", "Icon sources").children([
            sidebar_item("air-sym", "AirDrop · symbol").symbol("dot.radiowaves.right"),
            sidebar_item("air-art", "AirDrop · artwork").system_image("SidebarAirDrop"),
            sidebar_item("app-sym", "Applications · symbol").symbol("compass.drawing"),
            sidebar_item("app-art", "Applications · artwork")
                .system_image("SidebarApplicationsFolder"),
            sidebar_item("doc-sym", "Documents · symbol").symbol("doc"),
            sidebar_item("doc-art", "Documents · artwork").system_image("SidebarDocumentsFolder"),
        ]),
        sidebar_group("tags", "Tags").children([
            sidebar_item("red", "Red").symbol("circle.fill"),
            sidebar_item("unknown", "Unknown symbol").symbol("not.a.real.sf.symbol"),
            sidebar_item("unknown-art", "Unknown resource").system_image("SidebarNotAResource"),
            sidebar_item("untagged", "Untagged"),
        ]),
    ]
}

pub struct SidebarWindow {
    sidebar: Option<NativeSidebar>,
    options: SidebarOptions,
    selected: SharedString,
    moved: Option<SidebarMove>,
    ejected: Option<SharedString>,
}

impl SidebarWindow {
    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(window, cx, self.options.clone());
        sidebar.update(&items());
        sidebar.set_selected(Some(&self.selected));

        let entity = cx.entity().downgrade();
        let selecting = entity.clone();
        sidebar.on_select(move |id, _, cx| {
            let Some(id) = id.clone() else { return };
            selecting
                .update(cx, |this: &mut SidebarWindow, cx| {
                    this.selected = id;
                    cx.notify();
                })
                .ok();
        });
        let reordering = entity.clone();
        sidebar.on_reorder(move |moved, _, cx| {
            let moved = moved.clone();
            reordering
                .update(cx, |this: &mut SidebarWindow, cx| {
                    this.moved = Some(moved);
                    cx.notify();
                })
                .ok();
        });
        sidebar.on_eject(move |id, _, cx| {
            let id = id.clone();
            entity
                .update(cx, |this: &mut SidebarWindow, cx| {
                    this.ejected = Some(id);
                    cx.notify();
                })
                .ok();
        });
        self.sidebar = Some(sidebar);
    }

    // `collapsed` goes last: a thickness write reopens the pane, undoing an earlier collapse.
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

    fn header_height(&self) -> Option<f64> {
        let sidebar = self.sidebar.as_ref()?;
        sidebar.row_layout(0).map(|l| l.row.size.height)
    }
}

fn parent_of(moved: &SidebarMove) -> &str {
    moved.parent.as_ref().map_or("(root)", |id| id.as_ref())
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
                    .child(line(match &self.moved {
                        Some(m) => format!(
                            "on_reorder: {} → index {} in {}",
                            m.id,
                            m.index,
                            parent_of(m)
                        ),
                        None => "on_reorder: nothing dragged yet".into(),
                    }))
                    .child(line(match &self.ejected {
                        Some(id) => format!("on_eject: {id}"),
                        None => "on_eject: nothing ejected yet".into(),
                    }))
                    .child(line(
                        "Every row, its font, its icon box and the selection pill are AppKit's."
                            .into(),
                    )),
            ),
        )
    }
}

// Deferred: opening a window draws it before this returns, inside a listener holding Demo's lease.
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
            moved: None,
            ejected: None,
        });
        cx.notify();
    });
}

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

fn reopen(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) {
    if let Some(handle) = d.sidebar_page.window.take() {
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    }
    open(window, cx);
}

struct Live {
    probe: Option<SidebarProbe>,
    sample: Option<(f64, f64, f64, f64)>,
    header: Option<f64>,
    selected: SharedString,
    moved: Option<SidebarMove>,
    ejected: Option<SharedString>,
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

    let live = d.sidebar_page.window.and_then(|handle| {
        handle.read(cx).ok().map(|w| Live {
            probe: w.probe(),
            sample: w.sample_row(),
            header: w.header_height(),
            selected: w.selected.clone(),
            moved: w.moved.clone(),
            ejected: w.ejected.clone(),
        })
    });
    let open_label = if live.is_some() {
        "Bring the sidebar window forward"
    } else {
        "Open a window with a real sidebar"
    };
    let probe = live.as_ref().and_then(|l| l.probe);
    let sample = live.as_ref().and_then(|l| l.sample);
    let header = live.as_ref().and_then(|l| l.header);
    let selected = live.as_ref().map(|l| l.selected.clone());
    let moved = live.as_ref().and_then(|l| l.moved.clone());
    let ejected = live.and_then(|l| l.ejected);

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
                        "live: rowHeight {:.1} · style {} → {} · item row h {row:.1} · group \
                         header h {} · cell x {cell:.1} · icon {icon:.1}pt · label x {title:.1}",
                        p.row_height,
                        p.row_size_style,
                        p.effective_row_size_style,
                        header.map_or("—".into(), |h| format!("{h:.1}"))
                    ),
                    (Some(p), None) => format!(
                        "live: rowHeight {:.1} · style {} → {}",
                        p.row_height, p.row_size_style, p.effective_row_size_style
                    ),
                    _ => "live: open the window to read AppKit back".into(),
                }),
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
                caption(
                    "expand_groups is not symmetric: on, it expands every group now and on every \
                     insertion; off, it stops expanding new ones and leaves the open ones open. \
                     Collapse a group by hand to see the difference.",
                ),
                knob(
                    "reorderable",
                    reorderable.to_string(),
                    native_switch("reorderable", reorderable)
                        .on_change(cx.listener(|d: &mut Demo, value: &bool, window, cx| {
                            d.sidebar_page.reorderable = *value;
                            reopen(d, window, cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                caption(
                    "reorderable is read once at install, so this reopens the window. Watch what \
                     else changes with it: a table whose data source merely defines \
                     outlineView:pasteboardWriterForItem: commits the selection on mouse-up \
                     instead of mouse-down, so a draggable sidebar does not light up under a held \
                     finger and a fixed one does. AppKit asks respondsToSelector: and never calls \
                     the method, so this is true even when it would return nil.",
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
        section(
            "row glyphs — SidebarIcon",
            vec![
                caption(
                    "Two routes, and the choice is per row rather than a rule. \
                     SidebarIcon::Symbol is an SF Symbol; SidebarIcon::SystemResource is a \
                     monochrome image out of /System/Library/CoreServices/CoreTypes.bundle, drawn \
                     as a template so the row's selection colours still reach it, at the rep that \
                     matches the icon box for the current row size.",
                ),
                caption(
                    "The Icon sources group in the window shows both for the same three rows. \
                     Scored by normalised cross-correlation against a capture of macOS 26's own \
                     Finder sidebar: AirDrop takes the artwork, 0.892 against the closest public \
                     symbol's 0.597, and there is no `airdrop` symbol on this build at all. \
                     Applications takes the artwork, 0.961 against compass.drawing's 0.303. \
                     Documents takes the symbol, `doc` 0.802 against SidebarDocumentsFolder's \
                     0.076 — which is why drawing a whole sidebar from the bundle is the tempting \
                     simplification and is wrong in both directions.",
                ),
                caption(
                    "Both routes fail the same way: the Tags group carries a symbol name no macOS \
                     knows and a bundle resource that does not exist, and each renders as no icon \
                     with the label still lined up — the cell never hides its image view. The row \
                     with no icon at all is the one that moves, because AppKit has nothing to \
                     indent it from.",
                ),
                mono("sidebar_item(id, title).symbol(\"doc\")"),
                mono("sidebar_item(id, title).system_image(\"SidebarAirDrop\")"),
                mono("sidebar::system_template(resource, points) -> Option<NSImage>"),
            ],
        ),
        section(
            "drag and eject",
            vec![
                caption(
                    "Turn reorderable on above, then drag a row within its own group in the \
                     window. Cross-group drops are refused, which is what Finder does. Bakery in \
                     Locations is ejectable(true) — click its eject affordance.",
                ),
                caption(match &moved {
                    Some(m) => format!(
                        "on_reorder: {} → index {} in {}",
                        m.id,
                        m.index,
                        parent_of(m)
                    ),
                    None if reorderable => "on_reorder: nothing dragged yet".into(),
                    None => "on_reorder: reorderable is off, so nothing can be dragged".into(),
                }),
                caption(match &ejected {
                    Some(id) => format!(
                        "on_eject: {id}. Nothing was unmounted — the sidebar reports the click \
                         and does nothing else."
                    ),
                    None => "on_eject: nothing ejected yet".into(),
                }),
                caption(
                    "Both are reported by the handler, not inferred from the tree. A row can move \
                     on screen and report nothing, and the two are what tells those apart.",
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
                "on_reorder(Fn(&SidebarMove, ..))",
                moved.map_or("—".into(), |m| {
                    format!("{} → {} in {}", m.id, m.index, parent_of(&m))
                }),
            ),
            (
                "on_eject(Fn(&SharedString, ..))",
                ejected.map_or("—".into(), |id| id.to_string()),
            ),
            (
                "SidebarItem::symbol / system_image / icon",
                "SidebarIcon::Symbol and ::SystemResource, chosen per row".into(),
            ),
            (
                "SidebarItem::ejectable(bool)",
                "one row — Bakery, in Locations".into(),
            ),
            (
                "sidebar::CORE_TYPES_BUNDLE",
                native_gpui::sidebar::CORE_TYPES_BUNDLE.into(),
            ),
            (
                "reorderable",
                "install-only; the switch reopens the window".into(),
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
