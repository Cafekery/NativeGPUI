//! Real AppKit views hosted inside a GPUI window.
//!
//! GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between
//! releases. A hosted `NSSwitch` gets the macOS 26 liquid-glass toggle, and gets whatever
//! macOS 27 makes it, without this crate being touched. Anything drawn in GPUI shaders is
//! frozen on the day it was written.
//!
//! # How it fits together
//!
//! [`native_root`] wraps your root element once per window. It creates the [`NativeHost`] for
//! that window and, after your tree has painted, commits every native view's geometry in one
//! pass. Components ([`native_switch`], [`glass`], [`material`], …) are ordinary GPUI elements
//! that reserve layout space and place a pooled AppKit view into that space.
//!
//! # Two layers
//!
//! * [`Layer::Above`] — a subview of GPUI's Metal view. Renders on top of GPUI content, takes
//!   real mouse and keyboard input. This is where controls live. macOS 26 glass placed here
//!   samples and blurs GPUI's own rendered content.
//! * [`Layer::Below`] — a sibling inserted beneath GPUI's Metal view. GPUI content composites
//!   on top of it, so GPUI-drawn text and icons sit *over* the material. Nothing here can
//!   receive input. This is where window backgrounds and sidebar materials live, and it
//!   requires a non-opaque window — see [`window_options`].
//!
//! # Budget
//!
//! Cost is per hosted view per frame, and only for views whose geometry actually moved — the host
//! skips `setFrame` when it has not, which is the difference between a static screen costing
//! nothing and costing everything.
//!
//! A plain view is ~1.2 µs and a switch ~3.5 µs, so hundreds are comfortable and thousands are
//! reachable. `NSSlider` is the outlier at ~16 µs, five times a switch: a couple of dozen moving
//! sliders is the practical ceiling, while static ones are nearly free.
//!
//! Materials are the expensive thing and their cost lands in the WindowServer rather than this
//! process, so a CPU measurement will call them free. Budget roughly a couple of dozen
//! `withinWindow` [`material`]s or a few dozen [`glass`] panels before a 120 Hz frame slips, and
//! reach for [`glass_container`] when there are many — batching the passes is worth an order of
//! magnitude.
//!
//! # Long lists
//!
//! Use `uniform_list`, not `div().overflow_y_scroll()`. `uniform_list` paints only the visible
//! range, so only visible rows place a view and the pool stays bounded however long the list is.
//! A 400-row list carrying a switch and a slider per row holds [`NativeHost::len`] at 39 under
//! `uniform_list` and 811 under a plain scrolling `div` — the whole list materialised, past the
//! switch budget and far past the slider one. The bound holds under motion, not just at rest.

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
pub use bridge::root::{NativeRoot, native_root, window_options};
pub use chrome::window_chrome::*;
pub use chrome::{sidebar, window_chrome};
pub use controls::*;
pub use materials::*;
pub use system::tokens;
