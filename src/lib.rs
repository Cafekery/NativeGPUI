//! Real AppKit views hosted inside a GPUI window. GPUI owns layout; macOS owns the pixels of
//! anything whose design Apple changes between releases.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]

mod bridge;
mod chrome;
mod system;

pub mod controls;
pub mod materials;
pub mod quicklook;
pub mod surfaces;

pub use bridge::appkit::{fitting_size, fitting_size_keyed, focus_gpui};
pub use bridge::host::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue, Slot,
    Target, ValueKind, element_key, wire,
};
pub use bridge::menus::{
    MenuCommand, MenuSection, add_menu, extend_menu, install_menus, on_menu_state, prepend_menu,
    set_menu,
};
pub use bridge::root::{NativeRoot, native_root, window_options};
pub use chrome::sidebar::*;
pub use chrome::tabs::{add_tab, set_trailing_inset, tab_bar_height, window_tabs};
pub use chrome::window_chrome::*;
pub use chrome::{sidebar, window_chrome};
pub use controls::*;
pub use materials::*;
pub use quicklook::*;
pub use surfaces::*;
pub use system::thumbnails;
pub use system::tokens;
