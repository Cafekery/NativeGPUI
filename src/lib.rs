//! Real AppKit views hosted inside a GPUI window.
//!
//! GPUI owns layout; macOS owns the pixels of anything whose design Apple changes between
//! releases. A hosted `NSSwitch` becomes the macOS 27 toggle without this crate being touched.
//! Anything drawn in GPUI shaders is frozen on the day it was written.
//!
//! # Where things are
//!
//! [`controls`], [`materials`], [`sidebar`], [`window_chrome`], [`surfaces`] and [`tokens`] are
//! all reachable straight from the crate root, and everything in the first five is re-exported
//! there by name as well, so `use native_gpui::*` reaches it. [`tokens`] keeps its own namespace,
//! because `system_color`, `accent` and `system_font` are too generic to spill into yours.
//!
//! * [`controls`] — the hosted controls: [`native_switch`], [`native_slider`],
//!   [`native_button`], [`native_text_field`], [`native_popup`], [`native_progress`],
//!   [`native_stepper`], and the rest.
//! * [`materials`] — vibrancy ([`material`]) and macOS 26 liquid glass ([`glass`],
//!   [`glass_container`]).
//! * [`sidebar`] — a real `NSSplitViewItem` source list, [`sidebar::NativeSidebar`]. A window
//!   shell rather than an element: install once, then update per frame.
//! * [`window_chrome`] — the window itself: [`set_corner_radius`], [`set_toolbar`],
//!   [`window_flags`], [`cursor_region`], and the [`native_rect`] → [`native_window_rect`] →
//!   [`native_screen_rect`] coordinate recipe every transient surface anchors through.
//! * [`surfaces`] — popovers, sheets and the system share picker, each in an `NSWindow` of its
//!   own.
//! * [`tokens`] — the system's colours, fonts, symbols and file icons, so the parts GPUI draws
//!   itself match the parts macOS draws.
//! * The menu bar — [`install_menus`], [`add_menu`], [`extend_menu`]. No module of its own out
//!   here, and a GPUI app has no menu bar at all until one of these installs it.
//! * `bridge` — the view pool, the ObjC glue and the frame boundary. Private, because you meet
//!   it as [`native_root`], [`NativeHost`], [`Layer`] and [`element_key`].
//!
//! Two slices of this surface have no caller anywhere in the tree, and neither is a leftover.
//! Each component's element type — [`NativeSwitch`], [`NativeSlider`] and their siblings — is
//! reached only through its own constructor, so nothing ever names it. And part of the surface
//! exists to *verify* rather than to build: [`NativeSidebar::probe`], [`corner_radius_readback`],
//! [`toolbar_description`], [`tokens::cache_stats`] and [`share_service_count`] read state back
//! out of AppKit so that a check has something able to fail. Neither slice puts a control on
//! screen, and a pass that deletes public items for want of callers will take both.
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
//! # One background
//!
//! A window has **one** background colour, [`tokens::WINDOW_BACKGROUND`], and every opaque surface
//! in it resolves to that: the toolbar band, the content pane under it, and whatever an app paints
//! in the body. Two values are visible as a step — the band and the body are one surface
//! interrupted by a strip of chrome, and resolving them differently draws a line across the window
//! where no line exists.
//!
//! The exception is a sidebar, and it is the only one. A sidebar is vibrancy, not a colour: it
//! samples the desktop behind the window, which is what macOS does and what makes it read as
//! glass. [`SidebarOptions::backdrop`](sidebar::SidebarOptions::backdrop) is that vibrancy and not
//! the window's background — making it opaque to match the content side flattens the panel and
//! closes the step in the wrong place.
//!
//! So: two surfaces, never three. A sidebar pane in vibrancy, and everything else in one colour.
//! With [`sidebar::NativeSidebar`] the crate paints it for you, band included, so an app should
//! paint no background at all; without one, paint [`tokens::window_background`] and nothing else.
//!
//! # Writing your own control
//!
//! Nothing a component needs is private, so a control this crate does not have is an ordinary
//! GPUI [`Element`](gpui::Element) whose `request_layout` asks AppKit for the size and whose
//! `paint` does three things — derive the key, place the view, push what changed:
//!
//! ```ignore
//! fn paint(&mut self, id: Option<&GlobalElementId>, bounds: Bounds<Pixels>, /* … */) {
//!     let key = element_key(id);
//!     let mask = window.content_mask().bounds;
//!     let host = NativeHost::get(window, cx);
//!
//!     let box_ = host.place(key, Layer::Above, bounds, mask, |mtm, target| {
//!         let box_ = NSComboBox::new(mtm);
//!         wire(&box_, target, key);
//!         box_
//!     });
//!
//!     if box_.isEnabled() != self.enabled {
//!         box_.setEnabled(self.enabled);
//!     }
//!     host.on_change(key, ValueKind::Text, self.on_change.take());
//! }
//! ```
//!
//! [`element_key`] turns GPUI's own element identity into the pool key, so the view follows its
//! element through reorders and scrolling. [`NativeHost::place`] builds the view on the first
//! frame and returns the pooled one after that, so the closure runs once. [`wire`] points the
//! control's action at the host's [`Target`]; [`NativeHost::on_change`] names the accessor to
//! read, and [`NativeHost::on_event`] carries anything that is not a scalar value. Write a
//! property only when it differs from what the control already reports: the host skips
//! `setFrame` for geometry that has not moved, and a blind write per frame gives that back.
//!
//! Size it with [`fitting_size`], which asks AppKit once and caches. An axis it returns as `0.0`
//! is AppKit saying that axis is free, not that it is zero.
//!
//! # Budget
//!
//! Per hosted view per frame, and only for views whose geometry moved: the host skips `setFrame`
//! when it has not. A still window writes no geometry at all — measured at 42 hosted views over
//! 2400 frames, zero `setFrame`, zero `setHidden`, zero restack — and its whole cost is **one**
//! objc call for the window, not per view: [`NativeHost::place`] reads
//! `presentsWithTransaction` once a frame to know which presentation path is carrying it.
//!
//! Placing a view is cheap; **creating** one is not, and a scrolling list does the second. The
//! pool is keyed by element identity, so a `uniform_list` row entering the viewport is a new key
//! and gets a new AppKit view rather than a recycled one — measured at ~460 µs for a row of a
//! switch and a slider, of which ~380 µs is the control's own constructor and ~55 µs is adding
//! it to a live view tree. At 44 views scrolling 8 points per frame that churn is ~305 µs per
//! frame against ~180 µs for every frame write combined. Reuse a stable id for a row that keeps
//! its meaning, and the cost disappears.
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

// `bridge`, `chrome` and `system` are grouping directories, not paths a caller types: what they
// hold is re-exported below, so nothing is reachable only through a nested module path.
mod bridge;
mod chrome;
mod system;

pub mod controls;
pub mod materials;
pub mod surfaces;

pub use bridge::appkit::{fitting_size, fitting_size_keyed, focus_gpui};
pub use bridge::host::{
    ControlSize, Layer, NativeDispatch, NativeHandler, NativeHost, NativeRead, NativeValue, Target,
    ValueKind, element_key, wire,
};
pub use bridge::menus::{MenuCommand, MenuSection, add_menu, extend_menu, install_menus};
pub use bridge::root::{NativeRoot, native_root, window_options};
pub use chrome::sidebar::*;
pub use chrome::window_chrome::*;
pub use chrome::{sidebar, window_chrome};
pub use controls::*;
pub use materials::*;
pub use surfaces::*;
pub use system::tokens;
