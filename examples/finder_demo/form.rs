use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::thread::LocalKey;

use block2::RcBlock;
use gpui::{AnyElement, App, Div, FontWeight, Stateful, Window, canvas, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, ns_font, system_color, system_font_family};
use native_gpui::{ControlSize, native_label, native_root, native_rule, native_switch};
use objc2::{AnyThread, MainThreadOnly};
use objc2_app_kit::{NSImage, NSPopUpButton, NSTextField, NSWindow};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSRunLoop, NSSize, NSString};

use crate::frame::ns_window;

pub(crate) const MARGIN: f32 = 20.;
pub(crate) const INSET: f32 = 10.;
pub(crate) const ROW: f32 = 37.;
pub(crate) const BODY: f32 = 13.;
pub(crate) const LINE: f32 = 16.;
pub(crate) const GAP: f32 = 16.;
pub(crate) const SECTION: f32 = 30.;
pub(crate) const STACK: f32 = 10.;
pub(crate) const TITLE: f32 = LINE + STACK;
pub(crate) const TITLEBAR: f32 = 32.;
pub(crate) const BOTTOM: f32 = 20.;
pub(crate) const KEPT: usize = 512;
const RADIUS: f32 = 12.;
const INDENT: f32 = 20.;
const PANEL_TITLEBAR: f32 = 24.;

pub(crate) type Flip = Rc<dyn Fn(bool, &mut App)>;

pub(crate) fn key_of(parts: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn remembered<V: Copy>(
    seen: &'static LocalKey<RefCell<HashMap<u64, V>>>,
    key: u64,
    measure: impl FnOnce() -> V,
) -> V {
    if let Some(value) = seen.with_borrow(|seen| seen.get(&key).copied()) {
        return value;
    }
    let value = measure();
    seen.with_borrow_mut(|seen| {
        if seen.len() >= KEPT {
            seen.clear();
        }
        seen.insert(key, value);
    });
    value
}

pub(crate) fn text_width(text: &str) -> f32 {
    thread_local! {
        static SEEN: RefCell<HashMap<u64, f32>> = RefCell::default();
    }
    remembered(&SEEN, key_of(text), || {
        let mtm = MainThreadMarker::new().expect("laid out on the main thread");
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        field.setFont(ns_font(system_font_family(), px(BODY), FontWeight::NORMAL).as_deref());
        field.fittingSize().width as f32
    })
}

pub(crate) fn text_height(text: &str, size: f32, weight: FontWeight, width: f32) -> f32 {
    thread_local! {
        static SEEN: RefCell<HashMap<u64, f32>> = RefCell::default();
    }
    let key = key_of((text, size.to_bits(), weight.0.to_bits(), width.to_bits()));
    remembered(&SEEN, key, || {
        let mtm = MainThreadMarker::new().expect("laid out on the main thread");
        let field = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
        field.setFont(ns_font(system_font_family(), px(size), weight).as_deref());
        let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(width.into(), 1e6));
        field.cell().map_or(0., |cell| {
            cell.cellSizeForBounds(bounds).height.ceil() as f32
        })
    })
}

pub(crate) fn lines_height(text: &str, width: f32) -> f32 {
    (text_height(text, BODY, FontWeight::NORMAL, width) / LINE)
        .round()
        .max(1.)
        * LINE
}

pub(crate) fn section(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .pt(px(SECTION))
        .flex()
        .flex_col()
        .gap(px(STACK))
}

pub(crate) fn heading(title: &str) -> Div {
    div()
        .px(px(MARGIN + INSET))
        .child(native_label("title", title).weight(FontWeight::BOLD))
}

pub(crate) fn labelled(label: &str, color: SystemColor, value: impl IntoElement) -> AnyElement {
    div()
        .h(px(ROW))
        .px(px(INSET))
        .flex()
        .items_center()
        .gap(px(GAP))
        .child(
            div()
                .flex_none()
                .w(px(text_width(label)))
                .child(native_label("label", label).color(color)),
        )
        .child(div().flex_1().flex().justify_end().child(value))
        .into_any_element()
}

pub(crate) fn switch_row(title: &str, on: bool, change: Option<Flip>) -> AnyElement {
    let color = match change {
        Some(_) => SystemColor::Label,
        None => SystemColor::DisabledControlText,
    };
    let switch = native_switch("switch", on)
        .control_size(ControlSize::Mini)
        .enabled(change.is_some())
        .on_change(move |on, _, cx| {
            if let Some(change) = &change {
                change(*on, cx);
            }
        });
    labelled(title, color, switch)
}

/// AppKit sizes a pop-up to its widest row; a form's value hugs its chevron.
pub(crate) fn hugging(title: &str, icon: bool) -> f32 {
    native_gpui::fitting_size_keyed("NSPopUpButton.form_value", key_of((title, icon)), |mtm| {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        popup.setBordered(false);
        popup.addItemWithTitle(&NSString::from_str(title));
        if let Some(item) = popup.lastItem().filter(|_| icon) {
            let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(16., 16.));
            item.setImage(Some(&image));
        }
        popup.fittingSize()
    })
    .width as f32
}

pub(crate) struct Form {
    pub(crate) wide: f32,
    pub(crate) inner: f32,
    hair: f32,
}

impl Form {
    pub(crate) fn new(width: f32, hair: f32) -> Self {
        let wide = width - 2. * MARGIN;
        Form {
            wide,
            inner: wide - 2. * INSET,
            hair,
        }
    }

    pub(crate) fn room(&self, label: &str) -> f32 {
        self.inner - text_width(label) - GAP
    }

    pub(crate) fn card(&self, id: &'static str, rows: Vec<AnyElement>) -> Stateful<Div> {
        self.indented(id, rows.into_iter().map(|row| (0., row)))
    }

    /// The hairline under a row starts where that row's own content does.
    fn indented(
        &self,
        id: &'static str,
        rows: impl ExactSizeIterator<Item = (f32, AnyElement)>,
    ) -> Stateful<Div> {
        let last = rows.len().saturating_sub(1);
        div()
            .id(id)
            .mx(px(MARGIN))
            .rounded(px(RADIUS))
            .bg(system_color(SystemColor::QuaternarySystemFill))
            .children(rows.enumerate().map(|(at, (indent, row))| {
                div()
                    .id(at)
                    .relative()
                    .pl(px(indent))
                    .child(row)
                    .when(at < last, |row| {
                        row.child(
                            div()
                                .absolute()
                                .left(px(INSET + indent))
                                .right(px(INSET))
                                .bottom_0()
                                .h(px(self.hair))
                                .bg(system_color(SystemColor::Separator)),
                        )
                    })
            }))
    }
}

pub(crate) fn page(
    window: &Window,
    content: Vec<AnyElement>,
    fit: Option<impl Fn(&Window, f32) + 'static>,
) -> impl IntoElement {
    let titlebar = ns_window(window).map_or(TITLEBAR, |ns| {
        (ns.frame().size.height - ns.contentLayoutRect().size.height) as f32
    });
    native_root(
        div()
            .size_full()
            .relative()
            .bg(system_color(SystemColor::WindowBackground))
            .child(
                div()
                    .id("ng.finder.form.scroll")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(titlebar))
                    .bottom_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .relative()
                            .pb(px(BOTTOM))
                            .flex()
                            .flex_col()
                            .children(content)
                            .when_some(fit, |content, fit| {
                                content.child(
                                    canvas(
                                        |_, _, _| (),
                                        move |bounds, _, window, _| {
                                            fit(window, titlebar + f32::from(bounds.size.height));
                                        },
                                    )
                                    .absolute()
                                    .size_full(),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(titlebar))
                    .h(px(0.5))
                    .child(native_rule("ng.finder.form.titlebar")),
            ),
    )
}

/// A row of a panel's card at its own height, indented when it is an option of the row above.
pub(crate) struct Row {
    content: AnyElement,
    tall: f32,
    indent: f32,
}

impl Row {
    pub(crate) fn new(content: impl IntoElement) -> Self {
        Row {
            content: content.into_any_element(),
            tall: ROW,
            indent: 0.,
        }
    }

    pub(crate) fn tall(mut self, tall: f32) -> Self {
        self.tall = tall;
        self
    }

    pub(crate) fn nested(mut self) -> Self {
        self.indent = INDENT;
        self
    }
}

/// A utility panel's cards, summed up as they are added so the panel opens at its content's height.
pub(crate) struct Sheet {
    form: Form,
    parts: Vec<AnyElement>,
    tall: f32,
}

impl Sheet {
    pub(crate) fn new(width: f32, hair: f32) -> Self {
        Sheet {
            form: Form::new(width, hair),
            parts: Vec::new(),
            tall: PANEL_TITLEBAR + BOTTOM,
        }
    }

    /// A titled card opens a section; an untitled one sits just under the card above it.
    pub(crate) fn card(&mut self, id: &'static str, title: Option<&str>, rows: Vec<Row>) {
        let above = match (self.parts.is_empty(), title) {
            (true, _) => MARGIN,
            (false, Some(_)) => SECTION,
            (false, None) => STACK,
        };
        let rows_tall: f32 = rows.iter().map(|row| row.tall).sum();
        self.tall += above + title.map_or(0., |_| TITLE) + rows_tall;
        let rows = rows.into_iter().map(|row| {
            let content = div().h(px(row.tall)).child(row.content);
            (row.indent, content.into_any_element())
        });
        let card = self.form.indented("card", rows);
        self.parts.push(
            div()
                .id(id)
                .pt(px(above))
                .flex()
                .flex_col()
                .gap(px(STACK))
                .when_some(title, |part, title| part.child(heading(title)))
                .child(card)
                .into_any_element(),
        );
    }

    /// The panel's own button, trailing, under every card.
    pub(crate) fn button(&mut self, button: impl IntoElement, tall: f32) {
        self.tall += MARGIN + tall;
        self.parts.push(
            div()
                .pt(px(MARGIN))
                .px(px(MARGIN))
                .flex()
                .justify_end()
                .child(button)
                .into_any_element(),
        );
    }

    pub(crate) fn height(&self) -> f32 {
        self.tall
    }

    pub(crate) fn into_page(self, window: &Window) -> impl IntoElement {
        page(window, self.parts, Some(fit_panel))
    }
}

/// As tall as its content, or as its screen where that is shorter, and never below the screen.
fn fit_panel(window: &Window, tall: f32) {
    let Some(ns) = ns_window(window) else {
        return;
    };
    if panel_frame(&ns, tall).is_none() {
        return;
    }
    // Resized inside gpui's update, the window keeps drawing at its old size.
    let block = RcBlock::new(move || {
        if let Some(frame) = panel_frame(&ns, tall) {
            ns.setFrame_display(frame, true);
        }
    });
    unsafe { NSRunLoop::mainRunLoop().performBlock(&block) };
}

fn panel_frame(ns: &NSWindow, tall: f32) -> Option<NSRect> {
    let frame = ns.frame();
    let visible = ns.screen().map(|screen| screen.visibleFrame());
    let tall = f64::from(tall);
    let height = visible.map_or(tall, |visible| tall.min(visible.size.height));
    let top = frame.origin.y + frame.size.height;
    let bottom = visible.map_or(top - height, |visible| (top - height).max(visible.origin.y));
    if (frame.size.height - height).abs() < 0.5 && (frame.origin.y - bottom).abs() < 0.5 {
        return None;
    }
    Some(NSRect::new(
        NSPoint::new(frame.origin.x, bottom),
        NSSize::new(frame.size.width, height),
    ))
}
