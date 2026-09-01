//! The per-window pool of AppKit views, and the bridge that gets their events into GPUI.
//!
//! One [`NativeHost`] per GPUI window owns two container views — one above GPUI's Metal view, one
//! below — and a pool of hosted views keyed by GPUI element identity. Components call
//! [`NativeHost::place`] during `paint`; [`crate::native_root`] calls [`NativeHost::commit`] once
//! the whole tree has painted.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AsyncWindowContext, Bounds, Global, GlobalElementId, Pixels, Window, px,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSControl, NSControlSize, NSView, NSWindowOrderingMode};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect, NSString};
use objc2_quartz_core::CALayer;

use crate::bridge::appkit::{
    Container, adopt_key_path, gpui_view, metal_layer, presentation_lag, rects_equal, to_rect,
};

/// A stable pool key for an element, derived from GPUI's own element identity.
///
/// `GlobalElementId` is unique per element and stable across frames, so a hosted view follows its
/// element through reorders and scrolling with no id scheme of our own.
pub fn element_key(id: Option<&GlobalElementId>) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.expect("native components always return an ElementId")
        .hash(&mut hasher);
    hasher.finish()
}

/// Where a hosted view sits relative to GPUI's Metal content.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Layer {
    /// Above GPUI's content. Receives mouse and keyboard input. macOS 26 glass placed here
    /// samples and blurs the GPUI content beneath it.
    Above,
    /// Beneath GPUI's content, so GPUI paints over it. Cannot receive input — AppKit hit-tests
    /// the Metal view first and it covers the window. Needs a non-opaque window for anything
    /// that samples the desktop; see [`crate::window_options`].
    Below,
}

/// The system control sizes, shared by every hosted control that has a `controlSize`. `Small` is
/// whatever macOS currently thinks small is, which is the point of naming a size and not a value.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ControlSize {
    Mini,
    Small,
    #[default]
    Regular,
    Large,
    ExtraLarge,
}

impl From<ControlSize> for NSControlSize {
    fn from(size: ControlSize) -> Self {
        match size {
            ControlSize::Mini => NSControlSize::Mini,
            ControlSize::Small => NSControlSize::Small,
            ControlSize::Regular => NSControlSize::Regular,
            ControlSize::Large => NSControlSize::Large,
            ControlSize::ExtraLarge => NSControlSize::ExtraLarge,
        }
    }
}

/// What a control reports when it fires. Read at fire time, never later: the view may be gone
/// by the time the handler runs.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeValue {
    None,
    Bool(bool),
    Number(f64),
    Int(i64),
    Text(String),
}

/// Which value accessor a control's action should read.
///
/// These are AppKit's generic `NSControl` accessors, and a control is free to mean something else
/// by them:
///
/// * `integerValue` is `trunc(doubleValue)` on a value control, so on a 0..1 slider it is **0 for
///   everything the user can reach** except exactly 1.0 — and 0 is the usual initial value too, so
///   a round trip reads 0 → 0 and looks like a control that never moved.
/// * `integerValue` on `NSPopUpButton` is the cell's state: a constant 1 for every pop-up and 0
///   for every pull-down, whichever row is picked. `NSSegmentedControl` is **not** the same shape
///   — its `integerValue` tracks `selectedSegment` exactly, in SelectOne and SelectAny alike, so
///   [`ValueKind::Int`] is right there. In Momentary, `setSelectedSegment:` never sticks and the
///   read is always -1.
///
/// So this is for a control's own scalar value; a **selection, index, position or non-scalar** must
/// go through [`NativeHost::on_event`] with the real accessor.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ValueKind {
    /// Nothing; the action is the event. `NSButton` clicks.
    None,
    /// `integerValue != 0`. Correct for a switch or a two-state button.
    Bool,
    /// `doubleValue`. Correct for any continuous control — sliders, steppers, level indicators.
    Number,
    /// `integerValue`. Correct only where the control's value genuinely *is* an integer, such as
    /// a checkbox's `NSControlStateValue` including the mixed state. Never for a selection.
    Int,
    /// `stringValue`.
    Text,
}

/// A change handler. `Rc` because the dispatch table is cloned out before the handler runs, so a
/// handler that re-enters the host cannot deadlock the table.
pub type NativeHandler = Rc<dyn Fn(NativeValue, &mut Window, &mut App)>;

/// Reads a control's value **while the AppKit action is still on the stack**.
///
/// Some AppKit values live only for the duration of the action call — `NSPathControl`'s
/// `clickedPathItem` is nil by the time a deferred handler runs, a menu's clicked item likewise.
/// Handlers must stay deferred (see `Target`), so read and handling are separate stages.
pub type NativeRead = Rc<dyn Fn(&AnyObject) -> Box<dyn Any>>;

/// Receives whatever a [`NativeRead`] produced, on a later turn when GPUI is not borrowed.
pub type NativeDispatch = Rc<dyn Fn(Box<dyn Any>, &mut Window, &mut App)>;

struct Entry {
    /// The pool key whose lifetime this entry follows. Usually equal to the table key; a control
    /// with more than one event channel registers extra tags under the same owner.
    owner: u64,
    read: NativeRead,
    handler: NativeDispatch,
}

#[derive(Default)]
struct Dispatch {
    table: RefCell<HashMap<u64, Entry>>,
}

impl Dispatch {
    fn lookup(&self, tag: u64) -> Option<(NativeRead, NativeDispatch)> {
        self.table
            .borrow()
            .get(&tag)
            .map(|entry| (entry.read.clone(), entry.handler.clone()))
    }
}

impl ValueKind {
    /// The reader for one of the five common control values.
    ///
    /// One `Rc` per kind for the life of the thread, cloned rather than rebuilt. Components call
    /// this from `paint`, so allocating here is a heap allocation per control per frame whose only
    /// contents are a copy of `self`, freed on the next frame and replaced with an identical one.
    pub fn reader(self) -> NativeRead {
        thread_local! {
            static CACHE: RefCell<Vec<Option<NativeRead>>> = const { RefCell::new(Vec::new()) };
        }
        let index = self as usize;
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() <= index {
                cache.resize(index + 1, None);
            }
            cache[index]
                .get_or_insert_with(|| -> NativeRead { Rc::new(move |sender| self.read(sender)) })
                .clone()
        })
    }

    fn read(self, sender: &AnyObject) -> Box<dyn Any> {
        let value = match self {
            ValueKind::None => NativeValue::None,
            ValueKind::Bool => {
                let raw: isize = unsafe { msg_send![sender, integerValue] };
                NativeValue::Bool(raw != 0)
            }
            ValueKind::Number => {
                let raw: f64 = unsafe { msg_send![sender, doubleValue] };
                NativeValue::Number(raw)
            }
            ValueKind::Int => {
                let raw: isize = unsafe { msg_send![sender, integerValue] };
                NativeValue::Int(raw as i64)
            }
            ValueKind::Text => {
                let raw: Retained<NSString> = unsafe { msg_send![sender, stringValue] };
                NativeValue::Text(raw.to_string())
            }
        };
        Box::new(value)
    }
}

pub struct TargetIvars {
    dispatch: Rc<Dispatch>,
    cx: AsyncWindowContext,
}

define_class!(
    /// The single target/action sink for every control in one window.
    ///
    /// `setTarget:` is a *weak* reference, so this must outlive every control pointing at it —
    /// [`HostState`] retains it for the window's lifetime.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGTarget"]
    #[ivars = TargetIvars]
    pub struct Target;

    impl Target {
        #[unsafe(method(ngAction:))]
        fn ng_action(&self, sender: *mut AnyObject) {
            let ivars = self.ivars();
            let tag: isize = unsafe { msg_send![sender, tag] };
            let Some((read, dispatch)) = ivars.dispatch.lookup(tag as u64) else {
                return;
            };

            // Read now: transient AppKit values do not survive to the deferred handler.
            let value = read(unsafe { &*sender });

            // Deferred, never synchronous. An AppKit action can fire while GPUI already
            // holds its `App` borrow — any programmatic `performClick` reached from a listener
            // does — and a synchronous update then fails and drops the event permanently.
            ivars
                .cx
                .spawn(async move |cx| {
                    let _ = cx.update(|window, app| dispatch(value, window, app));
                })
                .detach();
        }
    }
);

impl Target {
    fn new(
        mtm: MainThreadMarker,
        dispatch: Rc<Dispatch>,
        cx: AsyncWindowContext,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { dispatch, cx });
        unsafe { msg_send![super(this), init] }
    }
}

/// Point a control's action at the host. Call this from a component's create closure.
pub fn wire(control: &NSControl, target: &Target, key: u64) {
    control.setTag(key as isize);
    unsafe {
        control.setTarget(Some(target.as_ref()));
        control.setAction(Some(sel!(ngAction:)));
    }
}

struct TerminusIvars {
    gpui: Retained<NSView>,
}

define_class!(
    /// The one member of the key view loop that is not a hosted control: the boundary Tab leaves
    /// AppKit through.
    ///
    /// GPUI's own view declines the job. Loop membership is `canBecomeKeyView`, and GPUI's view
    /// never overrides `acceptsFirstResponder`, so `NSView`'s default false makes it permanently
    /// invisible to Tab — false even on the view `makeFirstResponder:` then focuses successfully,
    /// because `makeFirstResponder:` does not consult `acceptsFirstResponder` at all. So pointing
    /// `nextKeyView` at GPUI's view writes a link every walk steps over, closing the loop into a
    /// ring of hosted views the keyboard can never leave.
    ///
    /// Zero-sized rather than hidden: hiding disqualifies a view from the loop, while an empty
    /// frame contains no point and so takes no mouse.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGKeyViewTerminus"]
    #[ivars = TerminusIvars]
    struct Terminus;

    impl Terminus {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // Deferred one turn: `makeFirstResponder:` assigns the window's first responder *after*
            // `becomeFirstResponder` returns, so a nested call is overwritten by the call that made
            // it and focus lands here instead.
            unsafe {
                let _: () = msg_send![
                    self,
                    performSelector: sel!(ngHandOff),
                    withObject: std::ptr::null::<AnyObject>(),
                    afterDelay: 0.0,
                ];
            }
            true
        }

        #[unsafe(method(ngHandOff))]
        fn hand_off(&self) {
            let Some(window) = self.window() else {
                return;
            };
            // A click can land inside the deferred turn. Handing over then would steal the focus
            // the user just chose.
            let holding = window.firstResponder().is_some_and(|responder| {
                std::ptr::eq(
                    &*responder as *const _ as *const (),
                    self as *const _ as *const (),
                )
            });
            if holding {
                window.makeFirstResponder(Some(&self.ivars().gpui));
            }
        }
    }
);

impl Terminus {
    fn new(mtm: MainThreadMarker, gpui: Retained<NSView>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TerminusIvars { gpui });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

/// One frame's geometry for a hosted view, as [`NativeHost::place`] computed it.
#[derive(Copy, Clone, PartialEq, Debug)]
struct Placement {
    clip_frame: NSRect,
    view_frame: NSRect,
    hidden: bool,
    /// The display tick this layout was painted on. A placement is owed to a **refresh** and not
    /// to a paint — see [`NativeHost::place`] — so this, and not its position in the queue, is
    /// what decides when it goes out.
    tick: u64,
}

/// The deepest queue `CAMetalLayer` can ask for: `maximumDrawableCount` accepts 2 or 3, so the lag
/// is 1 or 2.
const MAX_LAG: usize = 2;

/// One hosted view's placements computed but not yet written, oldest first — see
/// [`NativeHost::place`] for why they wait.
///
/// At most one placement per tick survives, and [`Queue::due`] leaves only those still owed to a
/// future refresh, so `MAX_LAG` of them can be waiting when the next one arrives.
#[derive(Default)]
struct Queue {
    entries: [Option<Placement>; MAX_LAG + 1],
    len: usize,
}

impl Queue {
    /// Take this paint's placement and give back the one due now, if any.
    fn advance(&mut self, placement: Placement, tick: u64, lag: usize) -> Option<Placement> {
        // Unreachable while ticks arrive, since `due` leaves at most `MAX_LAG` behind. Reachable
        // if GPUI's display link dies with paints still coming, and then the oldest is the one
        // that has waited long enough.
        let forced = (self.len == self.entries.len())
            .then(|| self.take_front())
            .flatten();
        match self.newest() {
            // Two paints on one tick reach the screen as one frame, so only the later is ever
            // presented and the earlier has no refresh of its own to be written for.
            Some(held) if held.tick == placement.tick => {
                self.entries[self.len - 1] = Some(placement)
            }
            _ => {
                self.entries[self.len] = Some(placement);
                self.len += 1;
            }
        }
        self.due(tick, lag).or(forced)
    }

    /// The newest placement GPUI has already presented by `tick`, dropping it and everything
    /// older.
    ///
    /// `None` while the queue is still filling, which is the right answer rather than a gap: a
    /// view that has just appeared has no history to show, and GPUI has not presented its row
    /// either.
    fn due(&mut self, tick: u64, lag: usize) -> Option<Placement> {
        let mut due = None;
        while self.len > 0 {
            let front = self.entries[0].expect("len counts the filled prefix");
            if front.tick.saturating_add(lag as u64) > tick {
                break;
            }
            due = self.take_front();
        }
        due
    }

    fn take_front(&mut self) -> Option<Placement> {
        let front = self.entries[0].take();
        self.entries.rotate_left(1);
        self.len = self.len.saturating_sub(1);
        front
    }

    fn newest(&self) -> Option<Placement> {
        self.len.checked_sub(1).and_then(|last| self.entries[last])
    }
}

struct Slot {
    /// Clip wrapper: frame is the visible part of the element, `clipsToBounds` on, so partial
    /// clipping at a scroll container's edge clips hit-testing with it.
    clip: Retained<Container>,
    view: Retained<NSView>,
    /// `Retained<V>` for the component's concrete class, so reuse is type-checked.
    typed: Box<dyn Any>,
    layer: Layer,
    /// Set when a component adopted the clip out of the layer container — a glass panel inside an
    /// `NSGlassEffectContainerView`. Stored so `commit`'s reorder pass preserves that adoption.
    parent: Option<Retained<NSView>>,
    /// What was last *written* to AppKit, which is the layout of the refresh GPUI is presenting.
    clip_frame: NSRect,
    view_frame: NSRect,
    hidden: bool,
    queued: Queue,
    generation: u64,
}

impl Slot {
    /// Write a placement to AppKit, skipping the properties that already hold it — see
    /// [`NativeHost::place`] for why the diff is against our own last intent.
    fn write(&mut self, placement: Placement) {
        if !rects_equal(self.clip_frame, placement.clip_frame) {
            self.clip.setFrame(placement.clip_frame);
            self.clip_frame = placement.clip_frame;
        }
        if !rects_equal(self.view_frame, placement.view_frame) {
            self.view.setFrame(placement.view_frame);
            self.view_frame = placement.view_frame;
        }
        if self.hidden != placement.hidden {
            self.clip.setHidden(placement.hidden);
            self.hidden = placement.hidden;
        }
    }
}

struct HostState {
    mtm: MainThreadMarker,
    above: Retained<Container>,
    below: Retained<Container>,
    /// Both ends of the key view loop; `commit` closes the ring through it.
    terminus: Retained<Terminus>,
    target: Retained<Target>,
    dispatch: Rc<Dispatch>,
    /// Only ever used to get one turn off the paint stack, so the tick chain can reach a
    /// `Window` — see `commit`.
    cx: AsyncWindowContext,
    slots: HashMap<u64, Slot>,
    /// Placement order this frame, and last frame's, so both of `commit`'s caches of paint order
    /// are rewritten only when it changed.
    order: Vec<u64>,
    last_order: Vec<u64>,
    /// Set when a slot was created, which is the one way those caches can be wrong while `order`
    /// is unchanged — see `commit`.
    recreated: bool,
    /// GPUI's Metal layer, kept to read `presentsWithTransaction` per frame — see
    /// `frame_lag`.
    metal: Option<Retained<CALayer>>,
    /// How many frames GPUI's presented content trails its own layout, read once from the Metal
    /// layer — see [`NativeHost::place`].
    lag: usize,
    /// `frame_lag` and the generation it was read for, so it costs one objc call per
    /// frame rather than one per hosted view.
    frame_lag: usize,
    frame_lag_at: u64,
    /// Display refreshes this host has observed.
    ///
    /// This is both the clock a placement is stamped with and the clock it comes due on, and those
    /// two roles agree only while the chain advancing it has run on every refresh a paint could
    /// have happened on — see [`NativeHost::tick`].
    tick: u64,
    /// Set while the tick chain is running, which is for as long as anything is hosted and not
    /// only while a placement is owed — see [`NativeHost::tick`].
    ticking: bool,
    generation: u64,
}

/// The per-window pool of hosted AppKit views. Cheap to clone.
#[derive(Clone)]
pub struct NativeHost(Rc<RefCell<HostState>>);

#[derive(Default)]
struct Hosts(HashMap<AnyWindowHandle, NativeHost>);
impl Global for Hosts {}

impl NativeHost {
    /// The host for this window, created on first use.
    pub fn get(window: &mut Window, cx: &mut App) -> Self {
        let handle = window.window_handle();
        if let Some(host) = cx
            .try_global::<Hosts>()
            .and_then(|hosts| hosts.0.get(&handle))
            .cloned()
        {
            return host;
        }
        let host = Self::new(window, cx);
        cx.default_global::<Hosts>().0.insert(handle, host.clone());
        host
    }

    fn new(window: &mut Window, cx: &mut App) -> Self {
        let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
        let parent = gpui_view(window);
        let frame = parent.bounds();

        // Hosting a control is not enough for the keyboard to reach it; see `adopt_key_path`.
        adopt_key_path(&parent);

        let above = Container::new(mtm, frame, false);
        above.autoresize();
        parent.addSubview(&above);

        // `frame` above is `parent.bounds()`, which is the right rect for `above` because that is
        // a subview of `parent`. `below` is a subview of `parent`'s SUPERVIEW, so it needs
        // `parent.frame()` — the same rectangle expressed in the space it is actually being put
        // into. The two are only equal while gpui's view sits at its superview's origin, which a
        // [`crate::sidebar::NativeSidebar`] breaks: the content pane holds gpui's view at a
        // pane-cancelling origin, so `bounds()` here lands every below view a sidebar's width to
        // the right of the GPUI content it is supposed to sit under.
        let below = Container::new(mtm, parent.frame(), false);
        below.autoresize();
        let content =
            unsafe { parent.superview() }.expect("gpui's view is always inside a content view");
        content.addSubview_positioned_relativeTo(&below, NSWindowOrderingMode::Below, None);

        let dispatch = Rc::new(Dispatch::default());
        let target = Target::new(mtm, dispatch.clone(), window.to_async(cx));

        let terminus = Terminus::new(mtm, parent.clone());
        above.addSubview(&terminus);

        // Pinned off: with it on the window will rederive the whole loop on any hierarchy change,
        // overwriting what `commit` wrote, and it derives over AppKit's own view tree — a hosted
        // field's next key view comes back as an `_NSCoreHostingView` inside that same field. It
        // still tabs between two stacked fields, so it looks like it works; what it cannot do is
        // end the loop, since GPUI's view is not eligible (see [`Terminus`]) and the walk wraps.
        if let Some(ns_window) = parent.window() {
            ns_window.setAutorecalculatesKeyViewLoop(false);
        }

        Self(Rc::new(RefCell::new(HostState {
            mtm,
            above,
            below,
            terminus,
            target,
            dispatch,
            cx: window.to_async(cx),
            slots: HashMap::new(),
            order: Vec::new(),
            last_order: Vec::new(),
            recreated: false,
            metal: metal_layer(&parent),
            lag: presentation_lag(&parent).min(MAX_LAG),
            frame_lag: 0,
            frame_lag_at: 0,
            tick: 0,
            ticking: false,
            generation: 1,
        })))
    }

    /// How many AppKit views the pool currently holds.
    ///
    /// The observable that proves pooling works — a 400-row list with a control each keeps this in
    /// the tens — and the cheapest way to catch a component that leaks a view per frame.
    pub fn len(&self) -> usize {
        self.0.borrow().slots.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many display refreshes of GPUI presentation lag this host is holding geometry back by
    /// — see [`NativeHost::place`]. Read from the Metal layer when the host was created, and
    /// clamped to `MAX_LAG`, which is the queue's depth.
    pub fn lag(&self) -> usize {
        self.0.borrow().lag
    }

    /// Override [`NativeHost::lag`].
    ///
    /// The control arm of the measurement that justifies the queue at all: `0` restores
    /// write-through placement, so a rig can read the same window with and without compensation
    /// instead of comparing two builds.
    pub fn set_lag(&self, lag: usize) {
        let state = &mut *self.0.borrow_mut();
        state.lag = lag.min(MAX_LAG);
        state.frame_lag_at = 0;
        let (tick, lag) = (state.tick, state.lag);
        for slot in state.slots.values_mut() {
            // A shallower lag makes placements due that were not, and `0` makes all of them due,
            // so the arm takes effect on the frame it is set rather than `lag` frames later.
            if let Some(due) = slot.queued.due(tick, lag) {
                slot.write(due);
            }
        }
    }

    /// The pool keys of the views `layer`'s container is stacking, back to front.
    ///
    /// The observable that proves z-order, and it has to come from AppKit: the pool's own `order`
    /// records what *should* be on screen and the whole class of bug here is the two disagreeing.
    /// Views a component adopted into a parent of its own are not in the layer container.
    pub fn stacking(&self, layer: Layer) -> Vec<u64> {
        let state = self.0.borrow();
        let container: &NSView = match layer {
            Layer::Above => &state.above,
            Layer::Below => &state.below,
        };
        let subviews = container.subviews();
        (0..subviews.count())
            .filter_map(|index| {
                let subview = Retained::as_ptr(&subviews.objectAtIndex(index));
                state.slots.iter().find_map(|(key, slot)| {
                    let clip = Retained::as_ptr(&slot.clip).cast::<NSView>();
                    (clip == subview).then_some(*key)
                })
            })
            .collect()
    }

    /// The pool keys in the order the key view loop actually visits them, walked from the terminus.
    ///
    /// [`NativeHost::stacking`]'s companion for the other cache of paint order, read the same way:
    /// from AppKit's `nextKeyView` links, not from what the pool believes it wrote. A view whose
    /// link was never rewritten drops out of the Tab run silently.
    pub fn key_loop(&self) -> Vec<u64> {
        let state = self.0.borrow();
        let start: &NSView = &state.terminus;
        let mut keys = Vec::new();
        let mut view = unsafe { start.nextKeyView() };
        while let Some(current) = view {
            if std::ptr::eq(&*current, start) || keys.len() > state.slots.len() {
                break;
            }
            let found = state.slots.iter().find_map(|(key, slot)| {
                (Retained::as_ptr(&slot.view) == Retained::as_ptr(&current)).then_some(*key)
            });
            match found {
                Some(key) => keys.push(key),
                None => break,
            }
            view = unsafe { current.nextKeyView() };
        }
        keys
    }

    /// Reserve `key`'s hosted view for this frame and return it, creating it on first sight.
    ///
    /// `bounds` is the element's layout and `mask` the GPUI content mask it is clipped to, both in
    /// logical points from the top-left of the content view — the containers' own space. Geometry
    /// is written only when it changed, the single biggest performance lever in this crate.
    ///
    /// That diff compares against the rect this host last *set*, never against `view.frame()`: one
    /// objc call per view per frame cheaper, and immune to a control that normalises what you hand
    /// it. `NSSlider` rounds its frame origin in `initWithFrame:`, so a read-back diff disagrees
    /// with its own intent once per slider before settling. Components owe the same rule to any
    /// property AppKit normalises or owns, where the cost is worse than cosmetic:
    /// `NSButton.keyEquivalent` set to `"\r"` reads back as the empty string forever, so a
    /// read-back diff rewrites it every frame. Compare against your own last intent, or ask the
    /// object that owns the fact.
    ///
    /// Geometry is written not for *this* frame but for the frame GPUI has already **presented**,
    /// `presentation_lag` refreshes back, which is what keeps a hosted view pinned to its GPUI row
    /// while the row moves. Each slot queues its placements and writes the one that falls out.
    ///
    /// Writing the current frame's layout instead puts a hosted view *ahead* of the GPUI content it
    /// sits on by exactly the presentation lag, so under motion it separates by `lag x velocity`
    /// and re-joins the moment motion stops: a glass panel slides off its own backdrop and the
    /// backdrop shows past its edges. Measured at a driven scroll of 4, 8 and 16 points per frame,
    /// the lead was exactly 2.000 frames at every velocity — 8.00, 16.00 and 32.00 points, the same
    /// integer multiple on every row of every capture — and writing the presented frame instead
    /// brings it to 0.000. It is an integer multiple and not a fraction because the cause is
    /// presentation *order*, not easing: disabling implicit Core Animation actions around these
    /// writes does not reduce it, and wrapping each write in its own `CATransaction` makes it worse
    /// by a frame, because a nested commit lands after the one the layout came in on.
    ///
    /// **The lag is counted in display refreshes, not in paints, and they are not the same
    /// number.** GPUI presents on every refresh while it has recent input, but it re-runs `paint`
    /// only when something is dirty, so a scroll driven by real events paints on some refreshes
    /// and not others. A queue that hands back "the placement from two paints ago" is therefore
    /// right only for a caller that paints on every refresh — which a rig advancing its own offset
    /// per frame does by construction and a trackpad scroll does not. Each placement carries the
    /// tick it was painted on and comes due `lag` ticks later, so a paint that skipped refreshes
    /// is still written for the refresh that shows it.
    ///
    /// The queue depth follows the layer, so the two move together and neither is a tuned constant.
    /// It caps at `MAX_LAG`, which is enough while `CAMetalLayer.maximumDrawableCount` accepts
    /// only 2 or 3; a future Core Animation allowing more would under-correct rather than misbehave.
    pub fn place<V: Message + AsRef<NSView> + 'static>(
        &self,
        key: u64,
        layer: Layer,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        new: impl FnOnce(MainThreadMarker, &Target) -> Retained<V>,
    ) -> Retained<V> {
        self.place_in(key, layer, None, bounds, mask, new)
    }

    /// [`NativeHost::place`], but parented somewhere other than the layer container.
    ///
    /// For a component nesting hosted views inside another AppKit view — glass panels inside an
    /// `NSGlassEffectContainerView`'s `contentView` — pass that view as `parent`. Stating it here
    /// rather than re-parenting afterwards is what keeps `commit`'s reorder pass from undoing it,
    /// and what lets the container batch — 400 glass panels go from 18 fps to 120.
    pub fn place_in<V: Message + AsRef<NSView> + 'static>(
        &self,
        key: u64,
        layer: Layer,
        parent: Option<&NSView>,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        new: impl FnOnce(MainThreadMarker, &Target) -> Retained<V>,
    ) -> Retained<V> {
        let state = &mut *self.0.borrow_mut();
        let (lag, tick) = (frame_lag(state), state.tick);
        let visible = clamped_intersection(bounds, mask);
        let clip_frame = to_rect(visible);
        let view_frame = NSRect::new(
            objc2_foundation::NSPoint::new(
                (bounds.origin.x - visible.origin.x).to_f64(),
                (bounds.origin.y - visible.origin.y).to_f64(),
            ),
            objc2_foundation::NSSize::new(bounds.size.width.to_f64(), bounds.size.height.to_f64()),
        );
        let hidden = visible.size.width <= px(0.) || visible.size.height <= px(0.);

        // One lookup, two questions: whether this key's view must be rebuilt, and whether a view is
        // about to be created — which is what both of `commit`'s caches key off.
        let creating = match state.slots.get(&key) {
            None => true,
            Some(slot) => {
                slot.layer != layer
                    || !slot.typed.is::<Retained<V>>()
                    || slot.parent.as_deref() != parent
            }
        };
        if creating {
            if let Some(slot) = state.slots.remove(&key) {
                slot.clip.removeFromSuperview();
            }
            state.recreated = true;
        }

        let slot = state.slots.entry(key).or_insert_with(|| {
            let clip = Container::new(state.mtm, clip_frame, true);
            let view = new(state.mtm, &state.target);
            clip.addSubview(view.as_ref());
            match parent {
                Some(parent) => parent.addSubview(&clip),
                None => match layer {
                    Layer::Above => state.above.addSubview(&clip),
                    Layer::Below => state.below.addSubview(&clip),
                },
            }
            Slot {
                clip,
                view: view.as_ref().retain(),
                typed: Box::new(view),
                layer,
                parent: parent.map(|parent| parent.retain()),
                clip_frame,
                view_frame: NSRect::ZERO,
                hidden: false,
                queued: Queue::default(),
                generation: 0,
            }
        });

        if let Some(due) = slot.queued.advance(
            Placement {
                clip_frame,
                view_frame,
                hidden,
                tick,
            },
            tick,
            lag,
        ) {
            slot.write(due);
        }
        slot.generation = state.generation;

        let view = slot
            .typed
            .downcast_ref::<Retained<V>>()
            .expect("slot type was checked above")
            .clone();
        state.order.push(key);
        view
    }

    /// Register (or clear) the handler a control's action dispatches to. Call every frame from the
    /// component's `paint`, so the handler closes over the current view state.
    ///
    /// A selection or a position must use [`NativeHost::on_event`] instead — see [`ValueKind`].
    pub fn on_change(&self, key: u64, kind: ValueKind, handler: Option<NativeHandler>) {
        let dispatch = handler.map(|handler| -> NativeDispatch {
            Rc::new(move |value, window, cx| {
                if let Ok(value) = value.downcast::<NativeValue>() {
                    handler(*value, window, cx);
                }
            })
        });
        self.on_event(key, kind.reader(), dispatch);
    }

    /// Register (or clear) a handler with a custom fire-time reader.
    ///
    /// Use this when the value is not one of [`ValueKind`]'s five, or is only valid while the
    /// AppKit action is on the stack — a clicked path item or menu item, an `NSColor`, an `NSDate`,
    /// a selection index. `read` runs synchronously inside the action; the handler still runs
    /// deferred, because GPUI may hold its `App` borrow at fire time.
    ///
    /// Reading in the handler instead is not safe: a repaint can land between action and handler,
    /// and `paint` will have pushed the app's stale value back into the control, so the handler
    /// reads the value it just clobbered.
    pub fn on_event(&self, key: u64, read: NativeRead, handler: Option<NativeDispatch>) {
        self.on_event_channel(key, key, read, handler);
    }

    /// [`NativeHost::on_event`] for a control with more than one event channel.
    ///
    /// A text field needs two: per-keystroke change from its delegate, submit from its action.
    /// Register the extra channel under a distinct `tag` (which the control must report from `-tag`
    /// when it fires that channel) but the same `owner` — the pool key — so the entry is retired
    /// with the view instead of leaking when the control goes away.
    pub fn on_event_channel(
        &self,
        owner: u64,
        tag: u64,
        read: NativeRead,
        handler: Option<NativeDispatch>,
    ) {
        let table = &self.0.borrow().dispatch.table;
        // `None` retires the channel rather than parking a handler-less entry. `ngAction:` needs a
        // handler to do anything, so such an entry can never fire, and registering one costs a
        // table insert and a reader allocation on every frame a control with no handler paints.
        match handler {
            None => {
                table.borrow_mut().remove(&tag);
            }
            Some(handler) => {
                table.borrow_mut().insert(
                    tag,
                    Entry {
                        owner,
                        read,
                        handler,
                    },
                );
            }
        }
    }

    /// Retire views that were not placed this frame, fix z-order if placement order changed, and
    /// rechain the key view loop if the hosted views did.
    pub fn commit(&self) {
        let state = &mut *self.0.borrow_mut();
        let generation = state.generation;

        let mut retired: Vec<u64> = Vec::new();
        // Read off the pass that is already visiting every slot: whether anything is still owed is
        // one of the two facts `chain_runs` decides on.
        let mut pending = false;
        state.slots.retain(|key, slot| {
            let keep = slot.generation == generation;
            if keep {
                pending |= slot.queued.len > 0;
            } else {
                slot.clip.removeFromSuperview();
                retired.push(*key);
            }
            keep
        });
        if !retired.is_empty() {
            state
                .dispatch
                .table
                .borrow_mut()
                .retain(|_, entry| !retired.contains(&entry.owner));
        }

        // Both passes below cache paint order, so both go stale on the same two events and share
        // one predicate. The order diff is the obvious one. The other is `place_in`'s rebuild
        // branch: a component swapping a key's layer, type or parent gets a brand new view under
        // the same pool key at the same place in paint order, so `order` comes out byte-identical
        // while the view changed. Adding a subview appends it rather than restacking it, so a
        // rebuilt clip lands topmost in its container instead of at its paint position, and the
        // key view link that pointed at the view it replaced now points at one that is gone.
        let restack = std::mem::take(&mut state.recreated) || state.order != state.last_order;

        if restack {
            for key in &state.order {
                let Some(slot) = state.slots.get(key) else {
                    continue;
                };
                let container: &NSView = match &slot.parent {
                    Some(parent) => parent,
                    None => match slot.layer {
                        Layer::Above => &state.above,
                        Layer::Below => &state.below,
                    },
                };
                container.addSubview_positioned_relativeTo(
                    &slot.clip,
                    NSWindowOrderingMode::Above,
                    None,
                );
            }
        }

        // Tab order is GPUI's element order: it survives the pool reordering views between frames
        // where a geometric derivation cannot — see `setAutorecalculatesKeyViewLoop` above for what
        // AppKit builds when asked to work it out itself.
        //
        // The ring runs terminus -> first hosted -> ... -> last hosted -> terminus, putting
        // [`Terminus`] on both sides of the hosted run: Tab off the last view reaches it going
        // forward, shift-Tab off the first going back, and either way the keyboard goes to GPUI.
        // Closing the ring among the hosted views would trap the keyboard inside them.
        if restack {
            let start: &NSView = &state.terminus;
            let mut previous = start;
            for key in &state.order {
                let Some(slot) = state.slots.get(key) else {
                    continue;
                };
                unsafe { previous.setNextKeyView(Some(&slot.view)) };
                previous = &slot.view;
            }
            // Nothing hosted leaves `previous` at the terminus, and a view linked to itself is a
            // loop AppKit walks forever.
            let close = if std::ptr::eq(previous, start) {
                None
            } else {
                Some(start)
            };
            unsafe { previous.setNextKeyView(close) };
        }

        std::mem::swap(&mut state.order, &mut state.last_order);
        state.order.clear();
        state.generation += 1;

        // The queues hold every placement owed to a refresh that has not happened yet. GPUI stops
        // drawing the instant nothing is dirty, so without this the tail of a scroll would never be
        // written and hosted views would sit `lag x final velocity` short of their rows for as long
        // as the window stood still — the one failure mode that is visible at rest rather than only
        // under motion. The same chain is also what counts refreshes during the scroll, which is
        // why it is armed for anything hosted at all rather than only for something owed.
        if !state.ticking && chain_runs(state.slots.len(), pending) {
            state.ticking = true;
            let host = self.clone();
            let cx = state.cx.clone();
            // Deferred, never here: this runs inside GPUI's paint, and `on_next_frame` has to be
            // registered from a turn where a `Window` is reachable.
            cx.spawn(async move |cx| {
                let armed = cx.update(|window, _| {
                    let host = host.clone();
                    window.on_next_frame(move |window, _| host.tick(window));
                });
                if armed.is_err() {
                    host.0.borrow_mut().ticking = false;
                }
            })
            .detach();
        }
    }

    /// Count one display refresh and write whatever GPUI has presented by it.
    ///
    /// `Window::on_next_frame` is the hook because GPUI runs those callbacks at the top of its
    /// display-link callback, on **every** refresh, before it asks whether anything is dirty and
    /// before it draws. So on a refresh that does paint, this has already counted that refresh by
    /// the time [`NativeHost::place`] reads the counter — and a refresh, not a paint, is what a
    /// placement is owed to. It is therefore both the clock the queue is due against and how the
    /// tail of a scroll plays out: `lag` ticks after GPUI's last frame is exactly when that frame
    /// reaches the screen, so that is when AppKit should be showing the same layout.
    ///
    /// **It re-arms for as long as anything is hosted, never only while something is owed.** The
    /// counter is a refresh clock, and a clock that stops when the queues drain has stopped
    /// counting refreshes. The queues drain whenever paints skip two refreshes running, and the
    /// only way back is `commit`'s deferred re-arm, which cannot land before the next turn: two
    /// paints that arrive while the chain is down are stamped with the same already-past refresh,
    /// [`Queue::advance`] takes them for one frame, and the earlier layout is dropped. AppKit then
    /// holds still for a refresh and moves by two refreshes' worth on the next one, which is a
    /// visible stutter and not a constant offset.
    ///
    /// A rig that paints on every refresh cannot see that, and not merely because it fails to
    /// reproduce it: painting every refresh keeps a placement owed at all times, which is the
    /// precondition that stops the chain from dropping in the first place.
    ///
    /// It costs one callback and one pass over the pool per refresh, on a display link GPUI runs
    /// unconditionally while the window is visible. GPUI stops that link while the window is not
    /// visible, so this stops with it — and so do paints, so the counter cannot fall behind the
    /// thing it counts.
    ///
    /// `state.lag` and not `frame_lag`: the in-transaction path only presents from inside a paint,
    /// and a placement it produced was written on the spot rather than queued.
    fn tick(&self, window: &Window) {
        let state = &mut *self.0.borrow_mut();
        state.tick += 1;
        let (tick, lag) = (state.tick, state.lag);
        let mut pending = false;
        for slot in state.slots.values_mut() {
            if let Some(due) = slot.queued.due(tick, lag) {
                slot.write(due);
            }
            pending |= slot.queued.len > 0;
        }
        state.ticking = chain_runs(state.slots.len(), pending);
        if state.ticking {
            let host = self.clone();
            window.on_next_frame(move |window, _| host.tick(window));
        }
    }
}

/// Whether the tick chain must be running, given how many views are hosted and whether any
/// placement is still owed to a future refresh.
///
/// **It answers on the pool alone, and deliberately ignores `owed`.** The chain is this host's
/// refresh counter as much as it is what drains the queues (see [`NativeHost::tick`]), and a
/// counter that switches off when the queues drain has stopped counting refreshes — so the state
/// the old shape switched off in, views hosted and nothing owed, is exactly the state the next
/// paint arrives in and needs a live clock for. `owed` stays in the signature because it is the
/// input this decision is most tempting to reintroduce.
fn chain_runs(hosted: usize, _owed: bool) -> bool {
    hosted > 0
}

/// How many frames *this* frame's presented content will trail its layout by.
///
/// Normally [`HostState::lag`], but GPUI has a second presentation path: when Core Animation asks
/// its layer to display — a live resize, a window becoming key — it presents inside the running
/// `CATransaction` instead of queueing a drawable, and that frame reaches the screen with the same
/// commit that carries our `setFrame`, so it trails by nothing. GPUI marks that path by turning
/// `CAMetalLayer.presentsWithTransaction` on for the duration of the draw and off again after, and
/// our `paint` runs inside it, so reading the flag here is reading which path is presenting us
/// rather than guessing from the window's state.
///
/// Cached per generation: one objc call per frame instead of one per hosted view.
fn frame_lag(state: &mut HostState) -> usize {
    if state.frame_lag_at == state.generation {
        return state.frame_lag;
    }
    let in_transaction = state.metal.as_ref().is_some_and(|layer| {
        let with_transaction: bool = unsafe { msg_send![&**layer, presentsWithTransaction] };
        with_transaction
    });
    state.frame_lag = if in_transaction { 0 } else { state.lag };
    state.frame_lag_at = state.generation;
    state.frame_lag
}

fn clamped_intersection(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Bounds<Pixels> {
    let x0 = a.origin.x.to_f64().max(b.origin.x.to_f64());
    let y0 = a.origin.y.to_f64().max(b.origin.y.to_f64());
    let x1 = (a.origin.x + a.size.width)
        .to_f64()
        .min((b.origin.x + b.size.width).to_f64());
    let y1 = (a.origin.y + a.size.height)
        .to_f64()
        .min((b.origin.y + b.size.height).to_f64());
    Bounds {
        origin: gpui::point(px(x0 as f32), px(y0 as f32)),
        size: gpui::size(px((x1 - x0).max(0.) as f32), px((y1 - y0).max(0.) as f32)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn mask_containing_bounds_leaves_them_alone() {
        let element = bounds(10., 20., 30., 40.);
        assert_eq!(
            clamped_intersection(element, bounds(0., 0., 100., 100.)),
            element
        );
    }

    #[test]
    fn bounds_containing_mask_clip_to_the_mask() {
        let mask = bounds(10., 20., 5., 6.);
        assert_eq!(clamped_intersection(bounds(0., 0., 100., 100.), mask), mask);
    }

    #[test]
    fn partial_overlap_keeps_only_the_shared_rect() {
        // A row scrolled halfway out of the top of its viewport.
        let clipped = clamped_intersection(bounds(0., 90., 50., 20.), bounds(0., 100., 50., 200.));
        assert_eq!(clipped, bounds(0., 100., 50., 10.));
    }

    #[test]
    fn disjoint_collapses_to_zero_rather_than_negative() {
        // A negative size reaches setFrame verbatim; the hidden check keys off zero.
        for mask in [bounds(0., 500., 50., 20.), bounds(500., 0., 20., 50.)] {
            let clipped = clamped_intersection(bounds(0., 0., 50., 50.), mask);
            assert!(clipped.size.width >= px(0.) && clipped.size.height >= px(0.));
            assert!(clipped.size.width <= px(0.) || clipped.size.height <= px(0.));
        }
    }

    #[test]
    fn edge_contact_is_empty_not_one_pixel() {
        let clipped = clamped_intersection(bounds(0., 0., 50., 50.), bounds(50., 0., 50., 50.));
        assert_eq!(clipped.size.width, px(0.));
    }

    #[test]
    fn fractional_bounds_survive_intersection() {
        // GPUI lays out on a half-point grid, so the clip must not round.
        let clipped = clamped_intersection(bounds(10.5, 0., 20.25, 8.), bounds(0., 0., 100., 100.));
        assert_eq!(clipped, bounds(10.5, 0., 20.25, 8.));
    }

    fn placement(y: f64, tick: u64) -> Placement {
        Placement {
            clip_frame: NSRect::new(
                objc2_foundation::NSPoint::new(0., y),
                objc2_foundation::NSSize::new(10., 10.),
            ),
            view_frame: NSRect::ZERO,
            hidden: false,
            tick,
        }
    }

    /// The whole point: at every refresh, what AppKit holds is the layout GPUI is presenting —
    /// the newest paint from `lag` refreshes ago.
    ///
    /// Swept over paint strides because paints and refreshes are the same count only for a caller
    /// that paints on every refresh, which a driven rig is and a real scroll is not. A queue that
    /// counted paints passed at stride 1 and, at stride 3, held a layout three refreshes too old.
    #[test]
    fn what_is_written_is_what_gpui_is_presenting_at_every_refresh() {
        const LAG: usize = 2;
        for stride in 1..=4u64 {
            let mut queue = Queue::default();
            let mut held: Option<Placement> = None;
            let mut painted: Vec<u64> = Vec::new();
            for tick in 0..24u64 {
                let due = if tick % stride == 0 {
                    painted.push(tick);
                    queue.advance(placement(tick as f64, tick), tick, LAG)
                } else {
                    queue.due(tick, LAG)
                };
                held = due.or(held);
                let presented = painted
                    .iter()
                    .rev()
                    .find(|&&paint| paint + LAG as u64 <= tick)
                    .copied();
                assert_eq!(
                    held.map(|placement| placement.clip_frame.origin.y as u64),
                    presented,
                    "stride {stride}, tick {tick}"
                );
            }
        }
    }

    /// The shape [`chain_runs`] replaced: alive only while a placement was owed.
    fn while_owed(hosted: usize, owed: bool) -> bool {
        hosted > 0 && owed
    }

    /// The decision itself, at the state the two shapes disagree on: views hosted, nothing owed.
    /// That is the state the old shape switched the clock off in, and the state the next paint
    /// arrives in.
    #[test]
    fn the_chain_runs_on_the_pool_alone() {
        assert!(chain_runs(1, false));
        assert!(!while_owed(1, false));
        assert!(!chain_runs(0, false) && !chain_runs(0, true));
    }

    /// The property the refresh clock exists for: a placement painted on refresh N is still stamped
    /// with refresh N after the tick chain has dropped and re-armed.
    ///
    /// Both arms run the same cadence — a scroll that hiccups for three refreshes, which is what
    /// drains the queues, since they only empty when paints skip two refreshes running. They differ
    /// only in which decision keeps the chain alive:
    ///
    /// * [`while_owed`] is the shape this replaced. The re-arm out of `commit` is deferred, so the
    ///   two paints that follow the hiccup are stamped with the same already-past refresh,
    ///   [`Queue::advance`] takes them for one frame, and the first of them is never written:
    ///   `written` loses the layout painted on refresh 13 and jumps 9 -> 14.
    /// * [`chain_runs`] is alive for as long as anything is hosted, and every painted layout is
    ///   written once, in order, whatever the deferred re-arm does.
    ///
    /// The delay is swept because the re-arm's latency is not ours to choose: it is however long
    /// GPUI's foreground executor takes to reach a turn holding a `Window`, and one refresh is the
    /// best case rather than a guarantee.
    #[test]
    fn a_placement_keeps_its_refresh_across_a_dropped_chain() {
        const LAG: u64 = 2;
        const REFRESHES: u64 = 30;
        // Dense, a three-refresh stall, dense again.
        let paints = |refresh: u64| refresh < 10 || (13..26).contains(&refresh);
        for delay in 1..=3u64 {
            let mut lost = Vec::new();
            for (shipped, policy) in [
                (false, while_owed as fn(usize, bool) -> bool),
                (true, chain_runs as fn(usize, bool) -> bool),
            ] {
                let mut queue = Queue::default();
                let mut clock = 0u64;
                let mut alive = true;
                let mut arm_at = None;
                let mut written = Vec::new();
                for refresh in 0..REFRESHES {
                    if arm_at == Some(refresh) {
                        (alive, arm_at) = (true, None);
                    }
                    if alive {
                        clock += 1;
                        if let Some(due) = queue.due(clock, LAG as usize) {
                            written.push(due.clip_frame.origin.y as u64);
                        }
                    }
                    if paints(refresh) {
                        let painted = placement(refresh as f64, clock);
                        if let Some(due) = queue.advance(painted, clock, LAG as usize) {
                            written.push(due.clip_frame.origin.y as u64);
                        }
                        if !alive && arm_at.is_none() {
                            arm_at = Some(refresh + delay);
                        }
                    }
                    alive = alive && policy(1, queue.len > 0);
                }
                let owed: Vec<u64> = (0..REFRESHES - LAG).filter(|r| paints(*r)).collect();
                let missing: Vec<u64> = owed
                    .iter()
                    .copied()
                    .filter(|refresh| !written.contains(refresh))
                    .collect();
                assert_eq!(written, {
                    let mut sorted = written.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    sorted
                });
                if shipped {
                    assert!(missing.is_empty(), "delay {delay}, dropped {missing:?}");
                } else {
                    lost = missing;
                }
            }
            // The other arm has to be able to fail, or the first proves nothing. One refresh of
            // re-arm latency is the case the old shape survives; two or more is not.
            assert_eq!(
                lost.is_empty(),
                delay == 1,
                "delay {delay}, dropped {lost:?}"
            );
        }
    }

    #[test]
    fn lag_zero_writes_through() {
        let mut queue = Queue::default();
        for tick in 0..4u64 {
            let due = queue
                .advance(placement(tick as f64, tick), tick, 0)
                .expect("no delay");
            assert_eq!(due.clip_frame.origin.y, tick as f64);
        }
    }

    /// A lag that shrinks mid-run — GPUI switching to an in-transaction present — must catch up on
    /// the next frame rather than replaying the two placements it was still holding.
    #[test]
    fn shrinking_lag_catches_up_instead_of_replaying() {
        let mut queue = Queue::default();
        queue.advance(placement(0., 0), 0, 2);
        queue.advance(placement(1., 1), 1, 2);
        let due = queue
            .advance(placement(2., 2), 2, 0)
            .expect("write-through");
        assert_eq!(due.clip_frame.origin.y, 2.);
    }

    /// Two paints on one refresh reach the screen as one frame, so the earlier one must not be
    /// written for a refresh of its own — and must not take a queue slot the next tick needs.
    #[test]
    fn a_second_paint_on_one_tick_replaces_the_first() {
        let mut queue = Queue::default();
        queue.advance(placement(0., 0), 0, 2);
        queue.advance(placement(99., 0), 0, 2);
        queue.advance(placement(1., 1), 1, 2);
        let due = queue
            .advance(placement(2., 2), 2, 2)
            .expect("tick 0 is due");
        assert_eq!(due.clip_frame.origin.y, 99.);
    }

    /// Paints with no refreshes between them — GPUI's display link stopped — must not overrun the
    /// array or drop the newest layout on the floor.
    #[test]
    fn paints_without_ticks_stay_within_the_queue() {
        let mut queue = Queue::default();
        let mut held = None;
        for paint in 0..8u64 {
            held = queue
                .advance(placement(paint as f64, paint), 0, 99)
                .or(held);
        }
        assert!(queue.len <= MAX_LAG + 1);
        assert_eq!(
            held.expect("the oldest goes out rather than being overwritten")
                .clip_frame
                .origin
                .y,
            4.
        );
    }
}
