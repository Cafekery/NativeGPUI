// POSITIVE ARM — every public path this crate promises must resolve. Must COMPILE.
//
// One line per thing a consumer is told to reach for, so a re-export quietly dropped from
// src/lib.rs surfaces here rather than in someone else's project. Nothing in this tree imports the
// crate by its own name, so no other check reads these paths at all.
#![allow(unused_imports)]

// The extension point: writing a control this crate does not have. `place` takes a `u64` pool key
// that only `element_key` derives from GPUI element identity, so all four have to be public
// together or the central promise of the crate is unusable from outside it.
use native_gpui::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue, Target,
    ValueKind, element_key, fitting_size, fitting_size_keyed, wire,
};

// The frame boundary and the window.
use native_gpui::{NativeRoot, focus_gpui, native_root, window_options};

// The menu bar, which a GPUI app does not have until one of these installs it.
use native_gpui::{MenuCommand, MenuSection, add_menu, extend_menu, install_menus};

// Controls.
use native_gpui::{
    ButtonBezel, CheckState, MenuEntry, MenuOrigin, MenuView, native_button, native_checkbox,
    native_color_well, native_context_menu, native_date_picker, native_level, native_path_control,
    native_popup, native_progress, native_radio, native_search_field, native_secure_field,
    native_segmented, native_slider, native_spinner, native_stepper, native_switch,
    native_text_field,
};

// Materials.
use native_gpui::{Blending, Material, RawMaterial, glass, glass_container, material};

// The sidebar, reachable from the root and through its own module.
use native_gpui::sidebar::SidebarRowSize;
use native_gpui::{
    NativeSidebar, PaneAlign, SidebarIcon, SidebarItem, SidebarMetrics, SidebarOptions,
    sidebar_group, sidebar_item, system_template,
};

// The window chrome, and the coordinate recipe transient surfaces anchor through.
use native_gpui::{
    Cursor, ToolbarItem, ToolbarStyle, WindowLevel, chrome_host, cursor_region, native_rect,
    native_screen_rect, native_window_rect, set_corner_radius, set_toolbar, window_flags,
};

// The transient surfaces, reachable from the root and through their own module.
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{
    NativePopover, NativeSheet, PopoverOptions, PopoverBehavior, ShareItem, SurfaceAnchor, native_anchor,
    present_popover, present_share_picker, present_sheet,
};

// System values keep their own namespace deliberately: `system_color`, `accent` and `system_font`
// are too generic to spill into a consumer's root.
use native_gpui::tokens::{
    Appearance, FileType, Symbol, SymbolColor, SystemColor, WINDOW_BACKGROUND, accent, file_icon,
    srgb, system_color, system_font, window_background,
};

fn main() {}
