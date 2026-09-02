# Adding a component

A hosted control is one file in `src/controls/`: a builder, a `layout`, a `paint`, and one macro line. [`switch.rs`](../src/controls/switch.rs) is the whole pattern in 70 lines; this walks through it.

## 1. The builder

```rust
pub fn native_switch(id: impl Into<ElementId>, on: bool) -> NativeSwitch {
    NativeSwitch { id: id.into(), on, enabled: true, on_change: None }
}

pub struct NativeSwitch {
    id: ElementId,
    on: bool,
    enabled: bool,
    on_change: Option<NativeHandler>,
}
```

The `id` field is required: `hosted_element!` reads it, and it is what makes the view survive across frames. Take the control's state as arguments and options as chained setters. `on_change` wraps the caller's closure into a `NativeHandler` that matches on the `NativeValue` variant the control reports:

```rust
pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
    self.on_change = Some(Rc::new(move |value, window, cx| {
        if let NativeValue::Bool(on) = value {
            handler(&on, window, cx);
        }
    }));
    self
}
```

## 2. `layout`: how big

Ask AppKit once and cache it. `fitted` turns a `fittingSize` into a `Style`; an axis AppKit reports as `0` means "free", and `fitted` lets layout fill it.

```rust
fn layout(&self) -> Style {
    fitted(fitting_size("NSSwitch", |mtm| NSSwitch::new(mtm).fittingSize()))
}
```

If the size depends on the control's own properties (a button's title, a control size), hash those inputs into `fitting_size_keyed`.

## 3. `paint`: place, push, wire

```rust
fn paint(&mut self, slot: Slot, _: &mut Window, _: &mut App) {
    let switch = slot.place(Layer::Above, |mtm, target| {
        let switch = NSSwitch::new(mtm);
        wire(&switch, target, slot.key);
        switch
    });
    let wanted = if self.on { NSControlStateValueOn } else { NSControlStateValueOff };
    if switch.state() != wanted {
        switch.setState(wanted);
    }
    if switch.isEnabled() != self.enabled {
        switch.setEnabled(self.enabled);
    }
    slot.on_change(ValueKind::Bool, self.on_change.take());
}
```

- `slot.place` returns the pooled view, creating it through the closure on first sight. The closure is the only place that runs once; `wire` points the control's action at the window's target with this element's key.
- Push only what changed. Every setter you call unconditionally is a layout pass and a redraw AppKit did not need.
- Never diff a property AppKit normalises or owns (`keyEquivalent = "\r"` reads back empty, `NSPopUpButton.integerValue` is cell state). Keep your own last-written value and compare against that instead.
- `slot.on_change(kind, handler)` registers the handler for this frame. `ValueKind` picks the accessor read when the action fires; for a value that is not a plain number or string, use `slot.on_event(read, handler)` with your own `NativeRead`.
- Read a control's value inside the action, dispatch to GPUI deferred; `on_change` already does both. Doing the read later loses transient values, doing the dispatch synchronously drops the event while GPUI holds its `App` borrow.

## 4. The macro

```rust
hosted_element!(NativeSwitch);
```

This implements `Element` and `IntoElement`: `request_layout` calls your `layout`, `paint` derives the `Slot` (pool key from the `GlobalElementId`, the window's host, bounds, clip mask) and calls your `paint`.

Components that need a different element shape, such as a container with children (`glass_container`) or an element that wraps a GPUI child (`native_context_menu`), implement `Element` by hand and use `NativeHost::place` / `place_in` directly.

## 5. Export it

- Declare the module and re-export the builder and type in [`src/controls/mod.rs`](../src/controls/mod.rs). `lib.rs` re-exports `controls::*`.
- Add the new public paths to `scripts/surface/pos.rs`; `scripts/surface.sh` fails the gate if a promised path stops resolving.

## 6. Show it in the storybook

- Add `pages/<name>.rs` with `pub struct State` and `pub fn render(d: &mut Demo, window, cx) -> AnyElement`, built from the helpers in [`examples/storybook/ui.rs`](../examples/storybook/ui.rs) (`stack`, `row`, `section`, `demo`, `tile`, `props`).
- Register it in `pages/mod.rs`, add a `Page` to `PAGES` and an arm to `Demo::body` in [`examples/storybook/main.rs`](../examples/storybook/main.rs), and a `State` field on `Demo`.
- `NG_PAGE=<id> cargo run --release --example storybook --features private-api` opens straight onto it. The header shows frames per second and the worst gap over the last second; scrolling should read 120 fps on a ProMotion display.

## 7. Run the gate

`./scripts/check.sh` must end in `== VERDICT: PASS`. It builds both feature configurations, runs the unit tests and the in-process probes, checks rustdoc under `-D warnings`, the public surface, and that `src/` and `examples/` produce zero warnings.

## Things that look wrong and are not

- A control's value is set on every paint. That is the design: the value is the state, and the state is what the handler wrote.
- Handlers are re-registered every paint. Registration is a hash-map insert; it is what lets a handler capture fresh state without a retained closure going stale.
- `fitting_size` creates a throwaway control. Once per class per process, and it is the only honest source of a system control's size.
