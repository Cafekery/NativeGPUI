//! The transient surfaces: an `NSPopover` on a GPUI anchor, an `NSWindow` sheet, the share picker.

use gpui::{
    App, Bounds, Context, MouseButton, MouseDownEvent, Render, Window, WindowBounds, div, point,
    prelude::*, px, rgb, size,
};
use native_gpui::surfaces::{
    NativePopover, NativeSheet, PopoverOptions, ShareItem, SharePicker, SharePickerEvent,
    SheetResponse, SurfaceAnchor, SurfaceEdge, native_anchor, present_popover,
    present_share_picker, present_sheet, share_service_count,
};
use native_gpui::{native_root, window_options};
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSControlStateValueOn, NSSwitch, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

struct Surfaces {
    anchor: SurfaceAnchor,
    popover: Option<NativePopover>,
    sheet: Option<NativeSheet>,
    popover_says: Option<bool>,
    sheet_ends: usize,
    last_response: Option<SheetResponse>,
    services: Option<usize>,
    shared: Option<String>,
}

impl Surfaces {
    fn open_popover(&mut self, edge: SurfaceEdge, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popover();
        let spec = PopoverOptions {
            size: size(px(240.), px(110.)),
            edge,
            ..Default::default()
        };
        let anchor = self.anchor.clone();
        let entity = cx.weak_entity();
        let popover = present_popover(window, cx, &anchor, spec, move |mtm, actions| {
            let view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(240., 110.)),
            );
            let label = NSTextField::labelWithString(
                &NSString::from_str("System popover, GPUI anchor"),
                mtm,
            );
            label.setFrame(NSRect::new(NSPoint::new(16., 76.), NSSize::new(212., 20.)));
            view.addSubview(&label);

            let toggle = NSSwitch::initWithFrame(
                NSSwitch::alloc(mtm),
                NSRect::new(NSPoint::new(16., 42.), NSSize::new(52., 26.)),
            );
            let read = toggle.clone();
            let says = entity.clone();
            actions.on_action(&toggle, move |_, app| {
                let on = read.state() == NSControlStateValueOn;
                let _ = says.update(app, |this: &mut Surfaces, cx| {
                    this.popover_says = Some(on);
                    cx.notify();
                });
            });
            view.addSubview(&toggle);

            let button = NSButton::initWithFrame(
                NSButton::alloc(mtm),
                NSRect::new(NSPoint::new(96., 42.), NSSize::new(128., 26.)),
            );
            button.setTitle(&NSString::from_str("Close"));
            button.setBezelStyle(NSBezelStyle::Push);
            actions.on_action(&button, move |_, app| {
                let _ = entity.update(app, |this: &mut Surfaces, cx| {
                    this.close_popover();
                    cx.notify();
                });
            });
            view.addSubview(&button);
            view
        });

        self.popover = popover
            .map(|popover| popover.on_close(window, cx, |_, _| println!("popoverDidClose:")));
    }

    fn close_popover(&mut self) {
        if let Some(popover) = self.popover.take() {
            popover.close();
        }
    }

    fn open_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.is_some() {
            return;
        }
        let entity = cx.weak_entity();
        let ended = entity.clone();
        self.sheet = Some(present_sheet(
            window,
            cx,
            move |mtm, actions| {
                let sheet = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        NSWindow::alloc(mtm),
                        NSRect::new(NSPoint::ZERO, NSSize::new(380., 160.)),
                        NSWindowStyleMask::Titled,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                let content = sheet
                    .contentView()
                    .expect("a titled window has a content view");
                let label = NSTextField::labelWithString(
                    &NSString::from_str("Real NSWindow sheet on the GPUI window"),
                    mtm,
                );
                label.setFrame(NSRect::new(NSPoint::new(24., 106.), NSSize::new(332., 20.)));
                content.addSubview(&label);

                for (title, response, x) in [
                    ("Cancel", SheetResponse::CANCEL, 132.),
                    ("Done", SheetResponse::OK, 252.),
                ] {
                    let button = NSButton::initWithFrame(
                        NSButton::alloc(mtm),
                        NSRect::new(NSPoint::new(x, 24.), NSSize::new(104., 28.)),
                    );
                    button.setTitle(&NSString::from_str(title));
                    button.setBezelStyle(NSBezelStyle::Push);
                    let entity = entity.clone();
                    actions.on_action(&button, move |_, app| {
                        let _ = entity.update(app, |this: &mut Surfaces, _| {
                            if let Some(sheet) = this.sheet.take() {
                                sheet.dismiss(response);
                            }
                        });
                    });
                    content.addSubview(&button);
                }
                sheet
            },
            move |response, _, app| {
                let _ = ended.update(app, |this: &mut Surfaces, cx| {
                    this.sheet_ends += 1;
                    this.last_response = Some(response);
                    cx.notify();
                });
            },
        ));
    }

    fn share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = vec![ShareItem::Url("https://example.com/native-gpui".into())];
        let services = share_service_count(&items);
        if services == 0 {
            println!("no share services for these items; the picker would open empty");
        }
        let anchor = self.anchor.clone();
        let entity = cx.weak_entity();
        present_share_picker(
            window,
            cx,
            &anchor,
            SharePicker::default(),
            items,
            move |event, _, app| match event {
                SharePickerEvent::Opening { services } => {
                    let _ = entity.update(app, |this: &mut Surfaces, cx| {
                        this.services = Some(services);
                        cx.notify();
                    });
                }
                SharePickerEvent::Presented { blocked } => println!(
                    "the show call held the main thread {:.4}s",
                    blocked.as_secs_f64()
                ),
                SharePickerEvent::Closed { chose } => {
                    let _ = entity.update(app, |this: &mut Surfaces, cx| {
                        this.shared = Some(chose.unwrap_or_else(|| "dismissed".to_owned()));
                        cx.notify();
                    });
                }
            },
        );
    }
}

impl Render for Surfaces {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (swatch, response) = match self.last_response {
            Some(SheetResponse::OK) => (rgb(0x30d158), "OK".to_owned()),
            Some(SheetResponse::CANCEL) => (rgb(0xff9f0a), "Cancel".to_owned()),
            Some(other) => (rgb(0x0a84ff), other.0.to_string()),
            None => (rgb(0x48484a), "-".to_owned()),
        };
        let status = format!(
            "sheet_ends={} last={response} popover_switch={} services={} shared={}",
            self.sheet_ends,
            match self.popover_says {
                Some(true) => "on",
                Some(false) => "off",
                None => "-",
            },
            self.services
                .map_or_else(|| "-".to_owned(), |count| count.to_string()),
            self.shared.as_deref().unwrap_or("-"),
        );

        let button =
            |label: &'static str,
             cx: &mut Context<Self>,
             action: fn(&mut Surfaces, &mut Window, &mut Context<Surfaces>)| {
                div()
                    .w(px(84.))
                    .h(px(32.))
                    .rounded(px(8.))
                    .bg(rgb(0x3a3a3c))
                    .border_1()
                    .border_color(rgb(0x545458))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xf2f2f7))
                    .text_size(px(11.))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this: &mut Surfaces, _: &MouseDownEvent, window, cx| {
                            action(this, window, cx);
                            cx.notify();
                        }),
                    )
            };

        native_root(
            div()
                .size_full()
                .bg(rgb(0x1c1c1e))
                .relative()
                .child(
                    div()
                        .absolute()
                        .left(px(32.))
                        .top(px(12.))
                        .flex()
                        .gap(px(6.))
                        .child(button("Pop above", cx, |this, window, cx| {
                            this.open_popover(SurfaceEdge::Above, window, cx)
                        }))
                        .child(button("Pop below", cx, |this, window, cx| {
                            this.open_popover(SurfaceEdge::Below, window, cx)
                        }))
                        .child(button("Close pop", cx, |this, _, _| this.close_popover()))
                        .child(button("Sheet", cx, Surfaces::open_sheet))
                        .child(button("Share", cx, Surfaces::share)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(32.))
                        .top(px(52.))
                        .size(px(28.))
                        .rounded(px(6.))
                        .bg(swatch),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(68.))
                        .top(px(58.))
                        .w(px(560.))
                        .text_color(rgb(0xf2f2f7))
                        .text_size(px(12.))
                        .child(status),
                )
                .child(native_anchor(
                    &self.anchor,
                    div()
                        .absolute()
                        .left(px(200.))
                        .top(px(200.))
                        .w(px(120.))
                        .h(px(32.))
                        .bg(rgb(0xff9f0a)),
                )),
        )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(200.), px(160.)),
                    size: size(px(700.), px(340.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|_| Surfaces {
                    anchor: SurfaceAnchor::new(),
                    popover: None,
                    sheet: None,
                    popover_says: None,
                    sheet_ends: 0,
                    last_response: None,
                    services: None,
                    shared: None,
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
