//! Real AppKit views hosted inside a GPUI window.
//!
//! GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between
//! releases. A hosted `NSSwitch` becomes the macOS 27 toggle without this crate being touched.
//! Anything drawn in GPUI shaders is frozen on the day it was written.
//!
//! # How it fits together
//!
//! [`native_root`] wraps your root element once per window, creates that window's [`NativeHost`],
//! and commits every native view's geometry in one pass after your tree paints. Components
//! ([`native_switch`], [`glass`], [`material`], …) are ordinary GPUI elements that reserve layout
//! space and place a pooled AppKit view into it.
//!
//! [`install_menus`] installs the standard menu bar — App, File, Edit, View, Window, Help — which
//! is where macOS keeps ⌘W, ⌘Q, ⌘M, ⌘H and the whole Edit menu. A GPUI app has no menu bar until
//! something asks for one, so until it does those chords reach nothing at all. Apps add their own
//! rows with [`add_menu`] and [`extend_menu`], which keep the standard set.
//!
//! The value you pass a component must be the state its `on_change` writes. A component pushes its
//! value on every paint, so a literal — `native_text_field("name", "")` wired to an `on_change`
//! that stores elsewhere — wipes the control a frame or two after each keystroke, once the pending
//! write drains. It reads as intermittent input loss rather than as a wiring mistake.
//!
//! # Two layers
//!
//! * [`Layer::Above`] — subview of GPUI's Metal view. Draws over GPUI content and takes input.
//!   Controls live here; macOS 26 glass placed here samples and blurs GPUI's own rendering.
//! * [`Layer::Below`] — sibling beneath the Metal view, so GPUI text and icons composite *over* it
//!   and nothing here can receive input. Window backgrounds and sidebar materials live here, and
//!   it needs a non-opaque window — see [`window_options`].
//!
//! # Budget
//!
//! Per hosted view per frame, and only for views whose geometry moved: the host skips `setFrame`
//! when it has not, which is why a static screen costs nothing.
//!
//! | | per frame |
//! |---|---|
//! | plain view | ~1.2 µs |
//! | switch | ~3.5 µs |
//! | slider | ~16 µs |
//!
//! Hundreds of switches are comfortable and thousands reachable. A couple of dozen *moving*
//! sliders is the ceiling; static ones are nearly free.
//!
//! Materials cost more, and it lands in the WindowServer rather than this process, so a CPU
//! measurement calls them free. Budget a couple of dozen `withinWindow` [`material`]s or a few
//! dozen [`glass`] panels per 120 Hz frame, and reach for [`glass_container`] when there are many
//! — batching is worth an order of magnitude.
//!
//! # Long lists
//!
//! Use `uniform_list`, not `div().overflow_y_scroll()`: only visible rows place a view, so the
//! pool stays bounded however long the list is. A 400-row list with a switch and a slider per row
//! holds [`NativeHost::len`] at 39 under `uniform_list` and 811 under a plain scrolling `div` —
//! the whole list materialised, past the switch budget and far past the slider one. The bound
//! holds under motion, not just at rest.

#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]

mod bridge;

pub mod chrome;
pub mod controls;
pub mod materials;
pub mod surfaces;
pub mod system;

pub use bridge::appkit::focus_gpui;
pub use bridge::host::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue,
    ValueKind, wire,
};
pub use bridge::menus::{MenuCommand, MenuSection, add_menu, extend_menu, install_menus};
pub use bridge::root::{NativeRoot, native_root, window_options};
pub use chrome::window_chrome::*;
pub use chrome::{sidebar, window_chrome};
pub use controls::*;
pub use materials::*;
pub use system::tokens;
