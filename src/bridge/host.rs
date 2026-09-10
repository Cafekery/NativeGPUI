use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AsyncWindowContext, Bounds, Global, GlobalElementId, Pixels, Window,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSControl, NSControlSize, NSView, NSWindowOrderingMode};
use objc2_foundation::{MainThreadMarker, NSObject, NSRect, NSString};

use crate::bridge::appkit::{Container, adopt_key_path, gpui_view, rects_equal, to_rect};

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
            // Deferred: an action can fire while GPUI already holds its `App` borrow.
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
    recreated: bool,
    generation: u64,
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
        cx.default_global::<Hosts>().0.insert(handle, host.clone());
        host
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

    pub fn len(&self) -> usize {
        self.0.borrow().slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

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
            Pooled {
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

        let view = slot
            .typed
            .downcast_ref::<Retained<V>>()
            .expect("slot type was checked above")
            .clone();
        state.order.push(key);
        view
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
        let table = &self.0.borrow().dispatch.table;
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

        // A rebuilt view lands topmost and drops out of the key loop, with `order` unchanged.
        if std::mem::take(&mut state.recreated) || state.order != state.last_order {
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

            let start: &NSView = &state.terminus;
            let mut previous = start;
            for key in &state.order {
                let Some(slot) = state.slots.get(key) else {
                    continue;
                };
                unsafe { previous.setNextKeyView(Some(&slot.view)) };
                previous = &slot.view;
            }
            let close = (!std::ptr::eq(previous, start)).then_some(start);
            unsafe { previous.setNextKeyView(close) };
        }

        std::mem::swap(&mut state.order, &mut state.last_order);
        state.order.clear();
        state.generation += 1;
    }
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
}
