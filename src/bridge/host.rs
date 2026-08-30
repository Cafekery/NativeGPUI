//! The per-window pool of AppKit views, and the bridge that gets their events into GPUI.
//!
//! One [`NativeHost`] exists per GPUI window. It owns two container views — one above GPUI's
//! Metal view, one below it — and a pool of hosted views keyed by GPUI element identity.
//! Components call [`NativeHost::place`] during `paint`; [`crate::native_root`] calls
//! [`NativeHost::commit`] once the whole tree has painted.

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

use crate::bridge::appkit::{Container, adopt_key_path, gpui_view, rects_equal, to_rect};

/// A stable pool key for an element, derived from GPUI's own element identity.
///
/// GPUI already guarantees `GlobalElementId` is unique per element and stable across frames, so
/// hosted views follow their element through reorders and scrolling without the caller inventing
/// an id scheme.
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

/// The system control sizes, shared by every hosted control that has a `controlSize`.
///
/// Ask for a size rather than a point value: `Small` is whatever macOS currently thinks small is.
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
/// * `integerValue` is `trunc(doubleValue)` on value controls, so on a 0..1 slider it is **0 for
///   everything the user can reach** except exactly 1.0 — and 0 is also the usual initial value,
///   so a round-trip test reads 0 → 0 and looks like a control that simply did not move.
/// * `integerValue` on `NSPopUpButton` is the cell's state: a constant 1 for every pop-up and 0
///   for every pull-down, whichever row is picked. `NSSegmentedControl` is the same shape.
///
/// So [`ValueKind`] is for a control's own scalar value. Anything that is a **selection, an index,
/// a position, or a non-scalar** must go through [`NativeHost::on_event`] with the real accessor.
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
/// Some AppKit values only exist for the duration of the action call — `NSPathControl`'s
/// `clickedPathItem` is nil by the time a deferred handler runs, and a menu's clicked item is the
/// same shape of problem. Handlers must stay deferred (see `Target`), so the read and the
/// handling are separate stages: read now, hand over later.
pub type NativeRead = Rc<dyn Fn(&AnyObject) -> Box<dyn Any>>;

/// Receives whatever a [`NativeRead`] produced, on a later turn when GPUI is not borrowed.
pub type NativeDispatch = Rc<dyn Fn(Box<dyn Any>, &mut Window, &mut App)>;

struct Entry {
    /// The pool key whose lifetime this entry follows. Usually equal to the table key; a control
    /// with more than one event channel registers extra tags under the same owner.
    owner: u64,
    read: NativeRead,
    handler: Option<NativeDispatch>,
}

#[derive(Default)]
struct Dispatch {
    table: RefCell<HashMap<u64, Entry>>,
}

impl Dispatch {
    fn lookup(&self, tag: u64) -> Option<(NativeRead, Option<NativeDispatch>)> {
        self.table
            .borrow()
            .get(&tag)
            .map(|entry| (entry.read.clone(), entry.handler.clone()))
    }
}

impl ValueKind {
    /// The reader for one of the five common control values.
    pub fn reader(self) -> NativeRead {
        Rc::new(move |sender| {
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
        })
    }
}

pub struct TargetIvars {
    dispatch: Rc<Dispatch>,
    cx: AsyncWindowContext,
}

define_class!(
    /// The single target/action sink for every control in one window.
    ///
    /// `setTarget:` is a *weak* reference, so this object must outlive every control that points
    /// at it — [`HostState`] retains it for the window's lifetime.
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
            let Some((read, Some(dispatch))) = ivars.dispatch.lookup(tag as u64) else {
                return;
            };

            // Read NOW, while the action is on the stack: transient AppKit values do not survive
            // to the deferred handler.
            let value = read(unsafe { &*sender });

            // Hand over LATER, deferred rather than synchronous. An AppKit action can fire while
            // GPUI already holds its `App` borrow — any programmatic `performClick` reached from a
            // listener does this — and a synchronous update then fails and drops the event
            // permanently.
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
    /// It exists because GPUI's own view declines the job. Loop membership is `canBecomeKeyView`,
    /// and GPUI's view never overrides `acceptsFirstResponder`, so `NSView`'s default false makes
    /// it permanently invisible to Tab — measured false on the very view `makeFirstResponder:`
    /// then focuses successfully, because `makeFirstResponder:` does not consult
    /// `acceptsFirstResponder` at all. Pointing `nextKeyView` straight at GPUI's view therefore
    /// writes a link every walk steps over, and the loop quietly closes into a ring of hosted
    /// views the keyboard can never leave.
    ///
    /// Zero-sized rather than hidden, because hiding a view disqualifies it from the loop, while
    /// an empty frame contains no point and so takes no mouse.
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
            // Deferred by one turn, not done here: `makeFirstResponder:` assigns the window's
            // first responder *after* `becomeFirstResponder` returns, so a nested call is
            // overwritten by the call that made it and focus lands on this view instead.
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

struct Slot {
    /// Clip wrapper: frame is the visible part of the element, `clipsToBounds` on. Gives correct
    /// partial clipping at a scroll container's edges, and clips hit-testing with it.
    clip: Retained<Container>,
    view: Retained<NSView>,
    /// `Retained<V>` for the component's concrete class, so reuse is type-checked.
    typed: Box<dyn Any>,
    layer: Layer,
    /// Where the clip wrapper is parented, when a component has adopted it out of the layer
    /// container — a glass panel inside an `NSGlassEffectContainerView`, for instance. `None` means
    /// the layer container. Stored so `commit`'s reorder pass preserves the adoption instead of
    /// pulling the view back out.
    parent: Option<Retained<NSView>>,
    clip_frame: NSRect,
    view_frame: NSRect,
    hidden: bool,
    generation: u64,
}

struct HostState {
    mtm: MainThreadMarker,
    above: Retained<Container>,
    below: Retained<Container>,
    /// Both ends of the key view loop: the hosted views chain into it, and it chains back into the
    /// first of them, so Tab and shift-Tab both leave the hosted run through GPUI.
    terminus: Retained<Terminus>,
    target: Retained<Target>,
    dispatch: Rc<Dispatch>,
    slots: HashMap<u64, Slot>,
    /// Placement order this frame, and last frame's, so both of `commit`'s caches of paint order
    /// are rewritten only when it changed.
    order: Vec<u64>,
    last_order: Vec<u64>,
    /// Set when a slot was created, which is the one way those caches can be wrong while `order`
    /// is unchanged — see `commit`.
    recreated: bool,
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

        // Hosting a control inside GPUI's view is not enough for the keyboard to reach it: this is
        // what puts GPUI's own view back into AppKit's key-handling path for the hosted subtree.
        adopt_key_path(&parent);

        let above = Container::new(mtm, frame, false);
        above.autoresize();
        parent.addSubview(&above);

        let below = Container::new(mtm, frame, false);
        below.autoresize();
        let content =
            unsafe { parent.superview() }.expect("gpui's view is always inside a content view");
        content.addSubview_positioned_relativeTo(&below, NSWindowOrderingMode::Below, None);

        let dispatch = Rc::new(Dispatch::default());
        let target = Target::new(mtm, dispatch.clone(), window.to_async(cx));

        let terminus = Terminus::new(mtm, parent.clone());
        above.addSubview(&terminus);

        // Pinned off, because with it on the window will rederive the whole loop on any hierarchy
        // change and overwrite what `commit` wrote — and the derivation is over AppKit's own view
        // tree, not over the views placed here. Measured with it on: a hosted field's next key view
        // came back as an `_NSCoreHostingView` *inside that same field*, and its previous key view
        // as the field's own clip container, which cannot become a key view. It still tabs between
        // two stacked fields, so it looks like it works; what it cannot do is end the loop, since
        // GPUI's view is not eligible (see [`Terminus`]) and the walk therefore wraps back into the
        // hosted views forever.
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
            slots: HashMap::new(),
            order: Vec::new(),
            last_order: Vec::new(),
            recreated: false,
            generation: 1,
        })))
    }

    /// How many AppKit views the pool currently holds.
    ///
    /// This is the observable that proves pooling works: GPUI only paints what is visible, so a
    /// scrolling list of 400 rows with a control each keeps this in the tens, not the hundreds. It
    /// is also the cheapest way to catch a component that leaks a view per frame.
    pub fn len(&self) -> usize {
        self.0.borrow().slots.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The pool keys of the views `layer`'s container is stacking, back to front.
    ///
    /// This is the observable that proves z-order, and it has to come from AppKit: the pool's own
    /// `order` records what *should* be on screen, and the whole class of bug here is the two
    /// disagreeing. Views a component adopted into a parent of its own are not in the layer
    /// container and so are not listed.
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
    /// The companion to [`NativeHost::stacking`] for the other cache of paint order, and read the
    /// same way: from AppKit's `nextKeyView` links rather than from what the pool believes it
    /// wrote. A view whose link was never rewritten drops out of the Tab run silently.
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
    /// `bounds` is the element's layout, `mask` the GPUI content mask it must be clipped to;
    /// both are logical points from the top-left of the window's content view, which is exactly
    /// the coordinate space the containers use. Geometry is written only when it changed — that
    /// diff is the single biggest performance lever in this crate.
    ///
    /// The diff compares against the rect this host last *set*, never against `view.frame()`.
    /// One objc call per view per frame cheaper, and immune by construction to a control that
    /// normalises what you hand it — `NSSlider` rounds its frame origin in `initWithFrame:`, so a
    /// read-back diff disagrees with its own intent once per slider before settling. Components
    /// must follow the same rule for any property AppKit normalises or owns, and there the
    /// consequence is not cosmetic: `NSButton.keyEquivalent` set to `"\r"` reads back as the empty
    /// string forever, so a read-back diff rewrites it every frame. Compare against your own last
    /// intent, or ask the object that owns the fact.
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
    /// For a component that nests hosted views inside another AppKit view — glass panels inside an
    /// `NSGlassEffectContainerView`'s `contentView`, which is what takes 400 panels from 18 fps to
    /// 120 — pass that view as `parent`. Stating it here rather than re-parenting after the fact is
    /// what keeps `commit`'s reorder pass from undoing it.
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

        // One lookup answers two questions: whether this key's view has to be rebuilt, and whether
        // a view is about to be created — which is what both of `commit`'s caches key off.
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
                generation: 0,
            }
        });

        if !rects_equal(slot.clip_frame, clip_frame) {
            slot.clip.setFrame(clip_frame);
            slot.clip_frame = clip_frame;
        }
        if !rects_equal(slot.view_frame, view_frame) {
            slot.view.setFrame(view_frame);
            slot.view_frame = view_frame;
        }
        if slot.hidden != hidden {
            slot.clip.setHidden(hidden);
            slot.hidden = hidden;
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
    /// Use this whenever the value is not one of [`ValueKind`]'s five, or is only valid while the
    /// AppKit action is on the stack — a clicked path item, a clicked menu item, an `NSColor`, an
    /// `NSDate`, a selection index. `read` runs synchronously inside the action; the handler still
    /// runs deferred, because GPUI may hold its `App` borrow at fire time.
    ///
    /// Reading in the handler instead is not a safe alternative: a repaint can land between the
    /// action and the handler, and `paint` will have pushed the app's stale value back into the
    /// control, so the handler reads the value it just clobbered.
    pub fn on_event(&self, key: u64, read: NativeRead, handler: Option<NativeDispatch>) {
        self.on_event_channel(key, key, read, handler);
    }

    /// [`NativeHost::on_event`] for a control with more than one event channel.
    ///
    /// A text field needs two: per-keystroke change from its delegate, and submit from its action.
    /// Register the extra channel under a distinct `tag` (which the control must report from
    /// `-tag` when it fires that channel) but the same `owner` — the pool key — so the entry is
    /// retired with the view instead of leaking when the control goes away.
    pub fn on_event_channel(
        &self,
        owner: u64,
        tag: u64,
        read: NativeRead,
        handler: Option<NativeDispatch>,
    ) {
        self.0.borrow().dispatch.table.borrow_mut().insert(
            tag,
            Entry {
                owner,
                read,
                handler,
            },
        );
    }

    /// Retire views that were not placed this frame, fix z-order if placement order changed, and
    /// rechain the key view loop if the hosted views did.
    pub fn commit(&self) {
        let state = &mut *self.0.borrow_mut();
        let generation = state.generation;

        let mut retired: Vec<u64> = Vec::new();
        state.slots.retain(|key, slot| {
            let keep = slot.generation == generation;
            if !keep {
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
        // one predicate. The order diff is the obvious event. The other is `place_in`'s stale
        // branch, where a component swapping a key's layer, type or parent gets a brand new view
        // under the same pool key at the same place in paint order, so `order` comes out
        // byte-identical while the view changed. Adding a subview appends it, so a rebuilt clip
        // lands topmost in its container rather than at its paint position, and the key view link
        // that pointed at the view it replaced now points at a view no longer in the tree.
        // Measured with the order diff alone: 101, 102, 103 placed, then 102's type swapped,
        // stacked 101 103 102 and left a key loop that stopped dead at 101.
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

        // Tab order is GPUI's element order, because that is what a reader perceives as
        // first-to-last, and it survives the pool reordering views between frames where a
        // geometric derivation cannot — see the note on `setAutorecalculatesKeyViewLoop` above for
        // what AppKit builds when asked to work it out itself.
        //
        // The ring runs terminus -> first hosted -> ... -> last hosted -> terminus, which puts
        // [`Terminus`] on both sides of the hosted run at once: Tab off the last view reaches it
        // going forward, shift-Tab off the first reaches it going back, and either way the
        // keyboard is handed to GPUI. Closing the ring among the hosted views instead would trap
        // it in whatever subset of the window happens to be AppKit.
        //
        // Only the visible views are in it, because only visible elements are placed — a hosted
        // control scrolled out of a `uniform_list` has no view for Tab to reach.
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
    }
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
        // GPUI lays out on a half-point grid and hosted frames are stored faithfully, so the
        // clip must not round.
        let clipped = clamped_intersection(bounds(10.5, 0., 20.25, 8.), bounds(0., 0., 100., 100.));
        assert_eq!(clipped, bounds(10.5, 0., 20.25, 8.));
    }
}
