//! The transient surfaces: `NSPopover`, a real `NSWindow` sheet, and `NSSharingServicePicker`.
//!
//! These are not builders. Each takes `&mut Window, &mut App` and presents there and then, so it
//! is called from a click handler rather than during render. Everything this page shows after a
//! click is GPUI painting state that only an AppKit callback ever wrote — the switch inside the
//! popover, the sheet's completion handler, the picker's delegate.
//!
//! A surface points at a GPUI element, whose geometry only exists during `paint`, so the element
//! is wrapped in `native_anchor` and the handler reads the bounds back out of the `SurfaceAnchor`.

use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window, div, prelude::*, px, size};
use native_gpui::surfaces::{
    NativePopover, NativeSheet, Popover, PopoverBehavior, ShareItem, SharePicker, SharePickerEvent,
    SheetResponse, SurfaceAnchor, SurfaceEdge, native_anchor, present_popover,
    present_share_picker, present_sheet, share_service_count,
};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{native_button, native_popup};
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSControlStateValueOn, NSSwitch, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// `PopoverBehavior`, in the order the picker offers them.
const BEHAVIORS: &[(&str, PopoverBehavior)] = &[
    ("ApplicationDefined", PopoverBehavior::ApplicationDefined),
    ("Transient", PopoverBehavior::Transient),
    ("Semitransient", PopoverBehavior::Semitransient),
];

#[derive(Default)]
pub struct State {
    pub anchor: SurfaceAnchor,
    pub share_anchor: SurfaceAnchor,
    pub popover: Option<NativePopover>,
    pub showing: Option<SurfaceEdge>,
    /// Index into [`BEHAVIORS`]; read when the popover is presented, so it takes effect next open.
    pub behavior: usize,
    /// Written only by the switch *inside* the popover.
    pub popover_says: Option<bool>,
    pub sheet: Option<NativeSheet>,
    pub sheet_ends: usize,
    pub last_response: Option<SheetResponse>,
    pub services: Option<usize>,
    pub blocked_ms: Option<f64>,
    pub shared: Option<String>,
    /// Asked once: `sharingServicesForItems:` is a real AppKit lookup and the answer for a fixed
    /// item list does not change.
    pub offered: Option<usize>,
}

fn share_items() -> Vec<ShareItem> {
    vec![
        ShareItem::Url("https://example.com/native-gpui".into()),
        ShareItem::Text("Shared from a GPUI window".into()),
    ]
}

fn open_popover(d: &mut Demo, edge: SurfaceEdge, window: &mut Window, cx: &mut Context<Demo>) {
    close_popover(d);

    let anchor = d.surfaces.anchor.clone();
    let spec = Popover {
        size: size(px(280.), px(132.)),
        edge,
        behavior: BEHAVIORS[d.surfaces.behavior.min(BEHAVIORS.len() - 1)].1,
        ..Default::default()
    };
    let entity = cx.weak_entity();
    let closed = entity.clone();

    let popover = present_popover(window, cx, &anchor, spec, move |mtm, actions| {
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(280., 132.)),
        );

        let title =
            NSTextField::labelWithString(&NSString::from_str("A real NSSwitch, in here"), mtm);
        title.setFrame(NSRect::new(NSPoint::new(16., 92.), NSSize::new(248., 20.)));
        view.addSubview(&title);

        let toggle = NSSwitch::initWithFrame(
            NSSwitch::alloc(mtm),
            NSRect::new(NSPoint::new(16., 48.), NSSize::new(52., 26.)),
        );
        let read = toggle.clone();
        let says = entity.clone();
        actions.on_action(&toggle, move |_, app| {
            let on = read.state() == NSControlStateValueOn;
            let _ = says.update(app, |d: &mut Demo, cx| {
                d.surfaces.popover_says = Some(on);
                cx.notify();
            });
        });
        view.addSubview(&toggle);

        let button = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            NSRect::new(NSPoint::new(148., 16.), NSSize::new(116., 28.)),
        );
        button.setTitle(&NSString::from_str("Close"));
        button.setBezelStyle(NSBezelStyle::Push);
        actions.on_action(&button, move |_, app| {
            let _ = entity.update(app, |d: &mut Demo, cx| {
                close_popover(d);
                cx.notify();
            });
        });
        view.addSubview(&button);

        view
    });

    d.surfaces.showing = popover.as_ref().map(|_| edge);
    d.surfaces.popover = popover.map(|popover| {
        popover.on_close(window, cx, move |_, app| {
            let _ = closed.update(app, |d: &mut Demo, cx| {
                d.surfaces.popover = None;
                d.surfaces.showing = None;
                cx.notify();
            });
        })
    });
    cx.notify();
}

fn close_popover(d: &mut Demo) {
    if let Some(popover) = d.surfaces.popover.take() {
        popover.close();
    }
    d.surfaces.showing = None;
}

fn open_sheet(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) {
    if d.surfaces.sheet.is_some() {
        return;
    }
    let entity = cx.weak_entity();
    let ended = entity.clone();

    d.surfaces.sheet = Some(present_sheet(
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
                &NSString::from_str("A real NSWindow, run as a sheet"),
                mtm,
            );
            label.setFrame(NSRect::new(NSPoint::new(24., 106.), NSSize::new(332., 20.)));
            content.addSubview(&label);

            // A third response proves `SheetResponse` is not limited to AppKit's own two.
            for (title, response, x) in [
                ("Cancel", SheetResponse::CANCEL, 24.),
                ("Later", SheetResponse(42), 148.),
                ("Done", SheetResponse::OK, 256.),
            ] {
                let button = NSButton::initWithFrame(
                    NSButton::alloc(mtm),
                    NSRect::new(NSPoint::new(x, 24.), NSSize::new(100., 28.)),
                );
                button.setTitle(&NSString::from_str(title));
                button.setBezelStyle(NSBezelStyle::Push);
                let entity = entity.clone();
                actions.on_action(&button, move |_, app| {
                    let _ = entity.update(app, |d: &mut Demo, _| {
                        if let Some(sheet) = d.surfaces.sheet.take() {
                            sheet.dismiss(response);
                        }
                    });
                });
                content.addSubview(&button);
            }
            sheet
        },
        move |response, _, app| {
            let _ = ended.update(app, |d: &mut Demo, cx| {
                d.surfaces.sheet_ends += 1;
                d.surfaces.last_response = Some(response);
                cx.notify();
            });
        },
    ));
    cx.notify();
}

fn open_share(d: &mut Demo, window: &mut Window, cx: &mut Context<Demo>) {
    let anchor = d.surfaces.share_anchor.clone();
    d.surfaces.shared = None;
    d.surfaces.blocked_ms = None;
    let entity = cx.weak_entity();

    present_share_picker(
        window,
        cx,
        &anchor,
        SharePicker {
            edge: SurfaceEdge::Below,
            ..Default::default()
        },
        share_items(),
        move |event, _, app| {
            let _ = entity.update(app, |d: &mut Demo, cx| {
                match event {
                    SharePickerEvent::Opening { services } => d.surfaces.services = Some(services),
                    SharePickerEvent::Presented { blocked } => {
                        d.surfaces.blocked_ms = Some(blocked.as_secs_f64() * 1000.)
                    }
                    SharePickerEvent::Closed { chose } => {
                        d.surfaces.shared = Some(chose.unwrap_or_else(|| "dismissed".to_owned()))
                    }
                }
                cx.notify();
            });
        },
    );
    cx.notify();
}

fn edge_button(id: &'static str, edge: SurfaceEdge, cx: &mut Context<Demo>) -> AnyElement {
    let title = match edge {
        SurfaceEdge::Above => "Above (MinY)",
        SurfaceEdge::Below => "Below (MaxY)",
        SurfaceEdge::Left => "Left (MinX)",
        SurfaceEdge::Right => "Right (MaxX)",
    };
    native_button(id, title)
        .on_click(cx.listener(move |d: &mut Demo, _: &(), window, cx| {
            open_popover(d, edge, window, cx);
        }))
        .into_any_element()
}

fn target() -> AnyElement {
    div()
        .w(px(200.))
        .h(px(44.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(system_color(SystemColor::Separator))
        .bg(system_color(SystemColor::QuaternaryLabel))
        .child(mono("popover anchor"))
        .into_any_element()
}

pub fn render(d: &mut Demo, _window: &mut Window, cx: &mut Context<Demo>) -> AnyElement {
    let anchor = d.surfaces.anchor.clone();
    let share_anchor = d.surfaces.share_anchor.clone();
    let showing = d.surfaces.showing;
    let popover_says = d.surfaces.popover_says;
    let behavior = d.surfaces.behavior.min(BEHAVIORS.len() - 1);
    let sheet_ends = d.surfaces.sheet_ends;
    let last_response = d.surfaces.last_response;
    let services = d.surfaces.services;
    let blocked_ms = d.surfaces.blocked_ms;
    let shared = d.surfaces.shared.clone();
    let offered = *d
        .surfaces
        .offered
        .get_or_insert_with(|| share_service_count(&share_items()));

    let bounds = anchor
        .bounds()
        .map(|b| {
            format!(
                "{:.0}×{:.0} at {:.0},{:.0}",
                b.size.width.to_f64(),
                b.size.height.to_f64(),
                b.origin.x.to_f64(),
                b.origin.y.to_f64()
            )
        })
        .unwrap_or_else(|| "not painted yet".to_owned());

    stack(vec![
        note(
            "Every surface here lives in its own NSWindow above the GPUI window, so its content \
             must be AppKit — GPUI renders a window's whole scene into the one Metal view it made.",
        ),
        demo(
            "anchor",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(native_anchor(&anchor, target()))
                .child(mono(format!("SurfaceAnchor::bounds() = {bounds}")))
                .child(caption(
                    "written during paint, so this line reports the previous frame",
                ))
                .into_any_element(),
        ),
        row(
            "every SurfaceEdge",
            vec![
                edge_button("edge-above", SurfaceEdge::Above, cx),
                edge_button("edge-below", SurfaceEdge::Below, cx),
                edge_button("edge-left", SurfaceEdge::Left, cx),
                edge_button("edge-right", SurfaceEdge::Right, cx),
            ],
        ),
        note(
            "Above is NSRectEdge MinY, which reads backwards until you know the edge is \
             interpreted in the positioning view's space: the chrome host is flipped, so the \
             anchor's MinY is its top and the popover renders above it.",
        ),
        knob(
            "behavior",
            BEHAVIORS[behavior].0,
            native_popup("popover-behavior", behavior)
                .items(BEHAVIORS.iter().map(|(name, _)| *name))
                .on_change(cx.listener(|d: &mut Demo, value: &usize, _, cx| {
                    d.surfaces.behavior = *value;
                    cx.notify();
                }))
                .into_any_element(),
        ),
        note(
            "ApplicationDefined is the default because it closes only on NativePopover::close, \
             and the other two self-dismiss on a click AppKit decides is outside — after which \
             your open flag disagrees with the screen unless on_close clears it. This page wires \
             on_close, so pick Transient, open a popover and click the background: the line below \
             goes back to closed on its own. Semitransient survives clicks inside this window and \
             closes on one outside it. Read on the next open, so choose before you present.",
        ),
        demo(
            "what the popover's switch wrote",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(mono(match showing {
                    Some(edge) => format!("showing on {edge:?}"),
                    None => "closed".to_owned(),
                }))
                .child(mono(match popover_says {
                    Some(on) => format!("NSSwitch says {on}"),
                    None => "NSSwitch untouched".to_owned(),
                }))
                .child(
                    native_button("popover-close", "Close from GPUI")
                        .enabled(showing.is_some())
                        .on_click(cx.listener(|d: &mut Demo, _: &(), _, cx| {
                            close_popover(d);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
                .into_any_element(),
        ),
        demo(
            "sheet",
            div()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    native_button("sheet-open", "Present sheet")
                        .on_click(cx.listener(|d: &mut Demo, _: &(), window, cx| {
                            open_sheet(d, window, cx);
                        }))
                        .into_any_element(),
                )
                .child(mono(format!("ended {sheet_ends}×")))
                .child(mono(match last_response {
                    Some(SheetResponse::OK) => "OK".to_owned(),
                    Some(SheetResponse::CANCEL) => "Cancel".to_owned(),
                    Some(other) => format!("SheetResponse({})", other.0),
                    None => "no response yet".to_owned(),
                }))
                .into_any_element(),
        ),
        demo(
            "share picker",
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(native_anchor(
                            &share_anchor,
                            native_button("share-open", "Share…").on_click(cx.listener(
                                |d: &mut Demo, _: &(), window, cx| open_share(d, window, cx),
                            )),
                        ))
                        .child(mono(format!("share_service_count() = {offered}"))),
                )
                .child(mono(match services {
                    Some(count) => format!("Opening with {count} services"),
                    None => "Opening: not seen".to_owned(),
                }))
                .child(mono(match blocked_ms {
                    Some(ms) => format!("Presented after blocking {ms:.2} ms"),
                    None => "Presented: not seen".to_owned(),
                }))
                .child(mono(match shared.as_deref() {
                    Some(chose) => format!("Closed, chose {chose}"),
                    None => "Closed: not seen".to_owned(),
                }))
                .into_any_element(),
        ),
        note(
            "The picker will not open for a background app: Opening still arrives and the call \
             still returns, but no window is created, and the Closed that never comes is the only \
             symptom. Its watchdog is what forces it shut if nobody dismisses it.",
        ),
        props(&[
            ("native_anchor(&SurfaceAnchor, child)", bounds),
            (
                "present_popover(window, cx, anchor, Popover, build)",
                match showing {
                    Some(edge) => format!("{edge:?}"),
                    None => "closed".into(),
                },
            ),
            (
                "Popover { size, edge, behavior, animates }",
                format!("280×132, {}", BEHAVIORS[behavior].0),
            ),
            (
                "PopoverBehavior",
                "ApplicationDefined (default) / Transient / Semitransient".into(),
            ),
            (
                "SurfaceEdge { Above, Below, Left, Right }",
                "MinY, MaxY, MinX, MaxX in the flipped host".into(),
            ),
            (
                "NativePopover::on_close / close",
                "on_close is what clears the flag above".into(),
            ),
            (
                "SurfaceActions::on_action(&NSControl, ..)",
                "the popover's switch and Close, the sheet's three buttons".into(),
            ),
            (
                "present_sheet(window, cx, build, on_end)",
                format!("{sheet_ends} completions"),
            ),
            (
                "SheetResponse::OK / CANCEL / other",
                "Done / Cancel / Later = 42".into(),
            ),
            (
                "present_share_picker(window, cx, anchor, SharePicker, items, on_event)",
                shared.unwrap_or_else(|| "not opened".into()),
            ),
            ("share_service_count(&[ShareItem])", offered.to_string()),
            ("SharePicker { edge, watchdog }", "Below, 20s".into()),
        ]),
    ])
}
