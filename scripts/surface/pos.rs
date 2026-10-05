// Positive arm: every public path this crate promises must resolve. Must COMPILE.
#![allow(unused_imports)]

use native_gpui::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue, Target,
    ValueKind, element_key, fitting_size, fitting_size_keyed, wire,
};

use native_gpui::{NativeRoot, focus_gpui, native_root, window_options};

use native_gpui::{MenuCommand, MenuSection, add_menu, extend_menu, install_menus, on_menu_state};

use native_gpui::{
    ButtonBezel, CheckState, LabelAlign, MenuEntry, MenuOrigin, MenuView, native_button,
    native_checkbox, native_color_well, native_context_menu, native_date_picker, native_label,
    native_level, native_path_control, native_popup, native_progress, native_radio, native_rule,
    native_scroll_bars, native_search_field, native_secure_field, native_segmented, native_slider,
    native_spinner, native_stepper, native_switch, native_text_field,
};

use native_gpui::{
    Blending, Material, RawMaterial, ScrollEdgeEffect, glass, glass_container, material,
    scroll_edge_effect,
};

use native_gpui::sidebar::SidebarRowSize;
use native_gpui::{
    NativeSidebar, PaneAlign, SidebarIcon, SidebarItem, SidebarMetrics, SidebarOptions,
    sidebar_group, sidebar_item, system_template,
};

use native_gpui::{
    Cursor, ToolbarItem, ToolbarStyle, WindowLevel, chrome_host, cursor_region, native_callout,
    native_rect, native_screen_rect, native_tooltip, native_window_rect, set_corner_radius,
    set_toolbar, window_flags,
};

use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{
    NativePopover, NativeSheet, PopoverBehavior, PopoverOptions, ShareItem, SurfaceAnchor,
    native_anchor, present_popover, present_share_picker, present_sheet,
};

use native_gpui::tokens::{
    Appearance, FileType, Symbol, SymbolColor, SystemColor, WINDOW_BACKGROUND, accent,
    draw_file_icon, file_icon, srgb, system_color, system_font, window_background,
};

fn main() {
    let _ = MenuCommand::checked;
    let _ = MenuEntry::file_icon;
    let _ = native_gpui::NativeSegmented::segment_enabled;
    let _ = ToolbarItem::selectable;
    let _ = ToolbarStyle::Preference;
}
