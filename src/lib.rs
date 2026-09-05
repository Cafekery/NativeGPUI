//! Real AppKit views hosted inside a GPUI window. GPUI owns layout; macOS owns the pixels of
//! anything whose design Apple changes between releases.
//!
//! [`native_root`] wraps a window's root element, creates that window's [`NativeHost`] and
//! commits every hosted view's geometry after the tree paints. Components ([`native_switch`],
//! [`glass`], [`material`], …) are ordinary GPUI elements that reserve layout space and place a
//! pooled AppKit view into it, keyed by GPUI element identity. [`install_menus`] installs the
//! standard menu bar, which is where ⌘W, ⌘Q, ⌘M, ⌘H and the Edit menu live.
//!
//! Two layers: [`Layer::Above`] is a subview of GPUI's Metal view, draws over GPUI content and
//! takes input; [`Layer::Below`] is a sibling beneath it, so GPUI composites over it and it can
//! never receive input. A window has one background colour, [`tokens::WINDOW_BACKGROUND`], and
//! one exception, the sidebar's vibrancy. The value passed to a component must be the state its
//! `on_change` writes, since a component pushes its value on every paint. Long lists must use
//! `uniform_list`, so only visible rows place a view.
//!
//! A control this crate does not have is an ordinary `Element` whose `request_layout` asks AppKit
//! for the size and whose `paint` derives the key with [`element_key`], calls
//! [`NativeHost::place`], pushes the properties that changed and registers a handler with
//! [`NativeHost::on_change`].

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
    MenuCommand, MenuSection, add_menu, extend_menu, install_menus, prepend_menu, set_menu,
};
pub use bridge::root::{NativeRoot, native_root, window_options};
pub use chrome::sidebar::*;
pub use chrome::tabs::{add_tab, window_tabs};
pub use chrome::window_chrome::*;
pub use chrome::{sidebar, window_chrome};
pub use controls::*;
pub use materials::*;
pub use quicklook::*;
pub use surfaces::*;
pub use system::thumbnails;
pub use system::tokens;
