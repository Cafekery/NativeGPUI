use std::any::Any;
use std::cell::{OnceCell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::{iter, ptr};

use gpui::{
    AnyWindowHandle, App, AsyncWindowContext, Bounds, EntityId, Global, GlobalElementId, Pixels,
    Window,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSControl, NSControlSize, NSView, NSWindowOrderingMode};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect, NSString};

use crate::bridge::appkit::{
    Container, adopt_hosted_accessibility, adopt_key_path, gpui_view, rects_equal, to_rect,
};

pub fn element_key(id: Option<&GlobalElementId>) -> u64 {
    let mut hasher = DefaultHasher::new();
    id.expect("native components always return an ElementId")
        .hash(&mut hasher);
    hasher.finish()
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Layer {
    Above,
    /// Beneath GPUI's view: GPUI paints over it and it never receives input.
    Below,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
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

#[derive(Clone, Debug, PartialEq)]
pub enum NativeValue {
    None,
    Bool(bool),
    Number(f64),
    Int(i64),
    Text(String),
}

/// `Int` is `integerValue`, which on a pop-up is the cell state rather than the selection.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ValueKind {
    None,
    Bool,
    Number,
    Int,
    Text,
}

pub type NativeHandler = Rc<dyn Fn(NativeValue, &mut Window, &mut App)>;
/// Runs inside the AppKit action, while transient values such as a clicked path item still exist.
pub type NativeRead = Rc<dyn Fn(&AnyObject) -> Box<dyn Any>>;
pub type NativeDispatch = Rc<dyn Fn(Box<dyn Any>, &mut Window, &mut App)>;

struct Entry {
    owner: u64,
    read: NativeRead,
    handler: NativeDispatch,
}

#[derive(Default)]
struct Dispatch {
    table: RefCell<HashMap<u64, Entry>>,
}

impl ValueKind {
    pub fn reader(self) -> NativeRead {
        thread_local! {
            static CACHE: [OnceCell<NativeRead>; 5] = Default::default();
        }
        CACHE.with(|cache| {
            cache[self as usize]
                .get_or_init(|| Rc::new(move |sender| self.read(sender)))
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
            ValueKind::Number => NativeValue::Number(unsafe { msg_send![sender, doubleValue] }),
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
            let entry = ivars.dispatch.table.borrow().get(&(tag as u64)).map(|entry| {
                (entry.read.clone(), entry.handler.clone())
            });
            let Some((read, dispatch)) = entry else {
                return;
            };
            let value = read(unsafe { &*sender });
            defer(&ivars.cx, move |window, app| dispatch(value, window, app));
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

/// Deferred a turn: AppKit can call back while gpui already holds its `App` borrow.
pub(crate) fn defer(cx: &AsyncWindowContext, run: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.spawn(async move |cx| {
        let _ = cx.update(run);
    })
    .detach();
}

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
    /// Hands Tab focus back to GPUI's view, which never accepts first responder itself.
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
            // `makeFirstResponder:` assigns after this returns and would overwrite a nested call.
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

struct Pooled {
    clip: Retained<Container>,
    view: Retained<NSView>,
    typed: Box<dyn Any>,
    layer: Layer,
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
    terminus: Retained<Terminus>,
    target: Retained<Target>,
    dispatch: Rc<Dispatch>,
    slots: HashMap<u64, Pooled>,
    order: Vec<u64>,
    last_order: Vec<u64>,
    created: Vec<u64>,
    kept: HashMap<EntityId, (Vec<u64>, u64)>,
    generation: u64,
}

impl HostState {
    fn layer_view(&self, layer: Layer) -> &NSView {
        match layer {
            Layer::Above => &self.above,
            Layer::Below => &self.below,
        }
    }
}

#[derive(Clone)]
pub struct NativeHost(Rc<RefCell<HostState>>);

#[derive(Default)]
struct Hosts(HashMap<AnyWindowHandle, NativeHost>);
impl Global for Hosts {}

impl NativeHost {
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
        if cx.try_global::<Hosts>().is_none() {
            // The terminus retains gpui's view, so an unreleased host keeps a closed window alive.
            cx.on_window_closed(|cx, closed| {
                cx.default_global::<Hosts>().0.retain(|handle, host| {
                    let open = handle.window_id() != closed;
                    if !open {
                        host.release();
                    }
                    open
                });
            })
            .detach();
        }
        cx.default_global::<Hosts>().0.insert(handle, host.clone());
        host
    }

    fn release(&self) {
        let Ok(mut state) = self.0.try_borrow_mut() else {
            return;
        };
        state.terminus.removeFromSuperview();
        state.above.removeFromSuperview();
        state.below.removeFromSuperview();
        state.dispatch.table.borrow_mut().clear();
        state.slots.clear();
    }

    fn new(window: &mut Window, cx: &mut App) -> Self {
        let mtm = MainThreadMarker::new().expect("gpui runs its windows on the main thread");
        let parent = gpui_view(window);
        adopt_key_path(&parent);

        let above = Container::new(mtm, parent.bounds(), false);
        above.autoresize();
        parent.addSubview(&above);

        // A sibling of gpui's view, so `frame()`: a sidebar offsets gpui's view.
        let below = Container::new(mtm, parent.frame(), false);
        below.autoresize();
        let content =
            unsafe { parent.superview() }.expect("gpui's view is always inside a content view");
        content.addSubview_positioned_relativeTo(&below, NSWindowOrderingMode::Below, None);

        let dispatch = Rc::new(Dispatch::default());
        let target = Target::new(mtm, dispatch.clone(), window.to_async(cx));
        let terminus = Terminus::new(mtm, parent.clone());
        above.addSubview(&terminus);

        // AppKit's own loop walks into hosted views' private subviews; `commit` writes it instead.
        if let Some(ns_window) = parent.window() {
            ns_window.setAutorecalculatesKeyViewLoop(false);
            if ns_window
                .contentView()
                .is_some_and(|view| ptr::eq(&*view, &*content))
            {
                adopt_hosted_accessibility(&content);
            }
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
            created: Vec::new(),
            kept: HashMap::new(),
            generation: 1,
        })))
    }

    pub fn len(&self) -> usize {
        self.0.borrow().slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn stacking(&self, layer: Layer) -> Vec<u64> {
        let state = self.0.borrow();
        let subviews = state.layer_view(layer).subviews();
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

    pub fn place_in<V: Message + AsRef<NSView> + 'static>(
        &self,
        key: u64,
        layer: Layer,
        parent: Option<&NSView>,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        new: impl FnOnce(MainThreadMarker, &Target) -> Retained<V>,
    ) -> Retained<V> {
        let view = self.place_between_frames(key, layer, parent, bounds, mask, new);
        self.0.borrow_mut().order.push(key);
        view
    }

    pub(crate) fn place_between_frames<V: Message + AsRef<NSView> + 'static>(
        &self,
        key: u64,
        layer: Layer,
        parent: Option<&NSView>,
        bounds: Bounds<Pixels>,
        mask: Bounds<Pixels>,
        new: impl FnOnce(MainThreadMarker, &Target) -> Retained<V>,
    ) -> Retained<V> {
        let state = &mut *self.0.borrow_mut();
        let visible = bounds.intersect(&mask);
        let clip_frame = to_rect(visible);
        let view_frame = to_rect(bounds - visible.origin);
        let hidden = visible.is_empty();

        let rebuild = state.slots.get(&key).is_some_and(|slot| {
            slot.layer != layer
                || !slot.typed.is::<Retained<V>>()
                || slot.parent.as_deref() != parent
        });
        if rebuild {
            state.slots.remove(&key).unwrap().clip.removeFromSuperview();
        }
        if !state.slots.contains_key(&key) {
            let clip = Container::new(state.mtm, clip_frame, true);
            let view = new(state.mtm, &state.target);
            clip.addSubview(view.as_ref());
            parent
                .unwrap_or_else(|| state.layer_view(layer))
                .addSubview(&clip);
            let pooled = Pooled {
                clip,
                view: view.as_ref().retain(),
                typed: Box::new(view),
                layer,
                parent: parent.map(|parent| parent.retain()),
                clip_frame,
                view_frame: NSRect::ZERO,
                hidden: false,
                generation: 0,
            };
            state.slots.insert(key, pooled);
            state.created.push(key);
        }
        let slot = state.slots.get_mut(&key).expect("inserted above");

        // Diffed against the last write, not read back: `NSSlider` rounds its frame origin.
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

        slot.typed
            .downcast_ref::<Retained<V>>()
            .expect("slot type was checked above")
            .clone()
    }

    pub fn on_change(&self, key: u64, kind: ValueKind, handler: Option<NativeHandler>) {
        self.on_change_channel(key, key, kind, handler);
    }

    pub(crate) fn on_change_channel(
        &self,
        owner: u64,
        tag: u64,
        kind: ValueKind,
        handler: Option<NativeHandler>,
    ) {
        let dispatch = handler.map(|handler| -> NativeDispatch {
            Rc::new(move |value, window, cx| {
                if let Ok(value) = value.downcast::<NativeValue>() {
                    handler(*value, window, cx);
                }
            })
        });
        self.on_event_channel(owner, tag, kind.reader(), dispatch);
    }

    pub fn on_event(&self, key: u64, read: NativeRead, handler: Option<NativeDispatch>) {
        self.on_event_channel(key, key, read, handler);
    }

    pub fn on_event_channel(
        &self,
        owner: u64,
        tag: u64,
        read: NativeRead,
        handler: Option<NativeDispatch>,
    ) {
        let state = self.0.borrow();
        let mut table = state.dispatch.table.borrow_mut();
        match handler {
            Some(handler) => {
                table.insert(
                    tag,
                    Entry {
                        owner,
                        read,
                        handler,
                    },
                );
            }
            None => {
                table.remove(&tag);
            }
        }
    }

    pub(crate) fn placed(&self) -> usize {
        self.0.borrow().order.len()
    }

    /// A cached view that placed nothing replayed its last paint: its views stay placed.
    pub(crate) fn keep(&self, owner: EntityId, from: usize, fresh: bool) {
        let HostState {
            slots,
            order,
            kept,
            generation,
            ..
        } = &mut *self.0.borrow_mut();
        if fresh || order.len() > from {
            kept.insert(owner, (order[from..].to_vec(), *generation));
            return;
        }
        let Some((keys, seen)) = kept.get_mut(&owner) else {
            return;
        };
        *seen = *generation;
        for key in keys.iter() {
            if let Some(slot) = slots.get_mut(key) {
                slot.generation = *generation;
                order.push(*key);
            }
        }
    }

    pub fn commit(&self) {
        let state = &mut *self.0.borrow_mut();
        let generation = state.generation;

        let mut retired = Vec::new();
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

        let created = std::mem::take(&mut state.created);
        if !created.is_empty() || state.order != state.last_order {
            restack(state, &created);
        }

        state.kept.retain(|_, (_, seen)| *seen == generation);
        std::mem::swap(&mut state.order, &mut state.last_order);
        state.order.clear();
        state.generation += 1;
    }
}

fn restack(state: &HostState, created: &[u64]) {
    let new = |key: &u64| created.contains(key);
    let container = |key: &u64| -> &NSView {
        let slot = &state.slots[key];
        slot.parent
            .as_deref()
            .unwrap_or_else(|| state.layer_view(slot.layer))
    };
    let order = &state.order;
    let survivors = state
        .last_order
        .iter()
        .filter(|key| state.slots.contains_key(key) && !new(key));
    if survivors.eq(order.iter().filter(|key| !new(key))) {
        for (at, key) in order.iter().enumerate().filter(|(_, key)| new(key)) {
            let home = container(key);
            let sibling = |other: &u64| ptr::eq(container(other), home);
            let Some((edge, neighbour)) = anchor(order, at, sibling, new) else {
                continue;
            };
            let neighbour: &NSView = &state.slots[&neighbour].clip;
            home.addSubview_positioned_relativeTo(&state.slots[key].clip, edge, Some(neighbour));
        }
    } else {
        for key in order {
            container(key).addSubview_positioned_relativeTo(
                &state.slots[key].clip,
                NSWindowOrderingMode::Above,
                None,
            );
        }
    }

    // AppKit relinks no key loop when a view leaves it: every link that differs is set.
    fn links(order: &[u64]) -> impl Iterator<Item = (Option<u64>, Option<u64>)> + '_ {
        let keys = order.iter().copied().map(Some);
        iter::once(None)
            .chain(keys.clone())
            .zip(keys.chain(iter::once(None)))
    }
    let before: HashSet<_> = links(&state.last_order).collect();
    let fresh = |key: Option<u64>| key.is_some_and(|key| new(&key));
    let view = |key: Option<u64>| -> &NSView {
        key.map_or(&state.terminus, |key| &state.slots[&key].view)
    };
    for (from, to) in links(order) {
        if !before.contains(&(from, to)) || fresh(from) || fresh(to) {
            let next = (to.is_some() || !order.is_empty()).then(|| view(to));
            unsafe { view(from).setNextKeyView(next) };
        }
    }
}

fn anchor(
    order: &[u64],
    at: usize,
    sibling: impl Fn(&u64) -> bool,
    new: impl Fn(&u64) -> bool,
) -> Option<(NSWindowOrderingMode, u64)> {
    if let Some(below) = order[..at].iter().rev().find(|key| sibling(key)) {
        return Some((NSWindowOrderingMode::Above, *below));
    }
    // `addSubview:` left each view created this frame topmost, wherever it belongs.
    let above = order[at + 1..]
        .iter()
        .find(|key| sibling(key) && !new(key))?;
    Some((NSWindowOrderingMode::Below, *above))
}

pub struct Slot<'a> {
    pub id: &'a GlobalElementId,
    pub key: u64,
    pub host: NativeHost,
    pub bounds: Bounds<Pixels>,
    pub mask: Bounds<Pixels>,
}

impl Slot<'_> {
    pub fn place<V: Message + AsRef<NSView> + 'static>(
        &self,
        layer: Layer,
        new: impl FnOnce(MainThreadMarker, &Target) -> Retained<V>,
    ) -> Retained<V> {
        self.host
            .place(self.key, layer, self.bounds, self.mask, new)
    }

    pub fn on_change(&self, kind: ValueKind, handler: Option<NativeHandler>) {
        self.host.on_change(self.key, kind, handler);
    }

    pub fn on_event(&self, read: NativeRead, handler: Option<NativeDispatch>) {
        self.host.on_event(self.key, read, handler);
    }
}

macro_rules! hosted_element {
    ($ty:ty) => {
        impl gpui::Element for $ty {
            type RequestLayoutState = ();
            type PrepaintState = ();

            fn id(&self) -> Option<gpui::ElementId> {
                Some(self.id.clone())
            }

            fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
                None
            }

            fn request_layout(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) -> (gpui::LayoutId, ()) {
                (window.request_layout(self.layout(), [], cx), ())
            }

            fn prepaint(
                &mut self,
                _: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                _: gpui::Bounds<gpui::Pixels>,
                _: &mut (),
                _: &mut gpui::Window,
                _: &mut gpui::App,
            ) {
            }

            fn paint(
                &mut self,
                id: Option<&gpui::GlobalElementId>,
                _: Option<&gpui::InspectorElementId>,
                bounds: gpui::Bounds<gpui::Pixels>,
                _: &mut (),
                _: &mut (),
                window: &mut gpui::Window,
                cx: &mut gpui::App,
            ) {
                let slot = $crate::bridge::host::Slot {
                    id: id.expect("native components always return an ElementId"),
                    key: $crate::bridge::host::element_key(id),
                    host: $crate::bridge::host::NativeHost::get(window, cx),
                    bounds,
                    mask: window.content_mask().bounds,
                };
                self.paint(slot, window, cx);
            }
        }

        impl gpui::IntoElement for $ty {
            type Element = Self;

            fn into_element(self) -> Self {
                self
            }
        }
    };
}
pub(crate) use hosted_element;

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};
    use objc2_app_kit::NSWindowOrderingMode;

    use super::anchor;

    #[test]
    fn a_mask_clear_of_the_bounds_leaves_an_empty_rect_not_a_negative_one() {
        let element = Bounds::new(point(px(0.), px(0.)), size(px(50.), px(50.)));
        for mask in [
            Bounds::new(point(px(0.), px(500.)), size(px(50.), px(20.))),
            Bounds::new(point(px(500.), px(0.)), size(px(20.), px(50.))),
            Bounds::new(point(px(50.), px(0.)), size(px(50.), px(50.))),
        ] {
            let visible = element.intersect(&mask);
            assert!(visible.is_empty());
            assert!(visible.size.width >= px(0.) && visible.size.height >= px(0.));
        }
    }

    #[test]
    fn views_created_around_older_ones_stack_in_paint_order() {
        let order = [1, 2, 10, 11, 3, 4, 12];
        let new = |key: &u64| *key < 10;
        let mut stack: Vec<u64> = order.iter().copied().filter(|key| !new(key)).collect();
        stack.extend(order.iter().copied().filter(new));
        for (at, key) in order.iter().enumerate().filter(|(_, key)| new(key)) {
            if let Some((edge, neighbour)) = anchor(&order, at, |_| true, new) {
                stack.retain(|other| other != key);
                let to = stack.iter().position(|other| *other == neighbour).unwrap();
                stack.insert(to + usize::from(edge == NSWindowOrderingMode::Above), *key);
            }
        }
        assert_eq!(stack, order);
    }
}
