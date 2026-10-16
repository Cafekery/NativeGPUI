use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::thread::LocalKey;

use block2::RcBlock;
use gpui::{AnyElement, App, Div, FontWeight, Stateful, Window, canvas, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, ns_font, system_color, system_font_family};
use native_gpui::{
    ControlSize, NativeLabel, native_label, native_root, native_rule, native_switch,
};
use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSControlSize, NSFont, NSImage, NSModalResponseOK, NSOpenPanel, NSPopUpButton, NSTextField,
    NSWindow,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURL};

use crate::chrome::toolbar::SYSTEM_RADIUS;
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
const PANEL_TITLEBAR: f32 = 24.;
const PANEL_MARGIN: f32 = 8.;

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
    text_width_at(text, BODY)
}

pub(crate) fn text_width_at(text: &str, size: f32) -> f32 {
    thread_local! {
        static SEEN: RefCell<HashMap<u64, f32>> = RefCell::default();
    }
    remembered(&SEEN, key_of((text, size.to_bits())), || {
        let mtm = MainThreadMarker::new().expect("laid out on the main thread");
        let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        field.setFont(ns_font(system_font_family(), px(size), FontWeight::NORMAL).as_deref());
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

#[derive(Copy, Clone, PartialEq)]
pub(crate) struct Density {
    row: f32,
    pub(crate) inset: f32,
    margin: f32,
    radius: f32,
    gap: f32,
    pub(crate) font: f32,
    pub(crate) line: f32,
    section: f32,
    stack: f32,
    indent: f32,
}

const REGULAR: Density = Density {
    row: ROW,
    inset: INSET,
    margin: MARGIN,
    radius: RADIUS,
    gap: GAP,
    font: BODY,
    line: LINE,
    section: SECTION,
    stack: STACK,
    indent: 20.,
};

pub(crate) const COMPACT: Density = Density {
    row: 26.,
    inset: 8.,
    margin: PANEL_MARGIN,
    radius: SYSTEM_RADIUS as f32 - PANEL_MARGIN,
    gap: 12.,
    font: 11.,
    line: 14.,
    section: 18.,
    stack: 8.,
    indent: 16.,
};

impl Density {
    pub(crate) fn label(self, id: &'static str, text: &str) -> NativeLabel {
        let label = native_label(id, text);
        if self == REGULAR {
            label
        } else {
            label.font_size(px(self.font))
        }
    }

    pub(crate) fn controls(self) -> ControlSize {
        if self == REGULAR {
            ControlSize::Regular
        } else {
            ControlSize::Small
        }
    }

    pub(crate) fn heading(self, title: &str) -> Div {
        div()
            .px(px(self.margin + self.inset))
            .child(self.label("title", title).weight(FontWeight::BOLD))
    }

    pub(crate) fn labelled(
        self,
        label: &str,
        color: SystemColor,
        value: impl IntoElement,
    ) -> AnyElement {
        div()
            .h(px(self.row))
            .px(px(self.inset))
            .flex()
            .items_center()
            .gap(px(self.gap))
            .child(
                div()
                    .flex_none()
                    .w(px(text_width_at(label, self.font)))
                    .child(self.label("label", label).color(color)),
            )
            .child(div().flex_1().flex().justify_end().child(value))
            .into_any_element()
    }

    pub(crate) fn switch_row(self, title: &str, on: bool, change: Option<Flip>) -> AnyElement {
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
        self.labelled(title, color, switch)
    }

    pub(crate) fn hugging(self, title: &str) -> f32 {
        hug(title, false, (self != REGULAR).then_some(self.font))
    }
}

pub(crate) fn labelled(label: &str, color: SystemColor, value: impl IntoElement) -> AnyElement {
    REGULAR.labelled(label, color, value)
}

pub(crate) fn switch_row(title: &str, on: bool, change: Option<Flip>) -> AnyElement {
    REGULAR.switch_row(title, on, change)
}

pub(crate) fn hugging(title: &str, icon: bool) -> f32 {
    hug(title, icon, None)
}

/// AppKit sizes a pop-up to its widest row; a form's value hugs its chevron.
fn hug(title: &str, icon: bool, small: Option<f32>) -> f32 {
    let key = key_of((title, icon, small.map(f32::to_bits)));
    native_gpui::fitting_size_keyed("NSPopUpButton.form_value", key, |mtm| {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
        popup.setBordered(false);
        if let Some(size) = small {
            popup.setControlSize(NSControlSize::Small);
            popup.setFont(Some(&NSFont::systemFontOfSize(size.into())));
        }
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
    density: Density,
}

impl Form {
    pub(crate) fn new(width: f32, hair: f32) -> Self {
        Form::at(REGULAR, width, hair)
    }

    fn at(density: Density, width: f32, hair: f32) -> Self {
        let wide = width - 2. * density.margin;
        Form {
            wide,
            inner: wide - 2. * density.inset,
            hair,
            density,
        }
    }

    pub(crate) fn room(&self, label: &str) -> f32 {
        self.inner - text_width(label) - GAP
    }

    pub(crate) fn card(&self, id: &'static str, rows: Vec<AnyElement>) -> Stateful<Div> {
        self.shell(id).children(self.rows(rows))
    }

    pub(crate) fn shell(&self, id: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .mx(px(self.density.margin))
            .rounded(px(self.density.radius))
            .bg(system_color(SystemColor::QuaternarySystemFill))
    }

    pub(crate) fn rows(&self, rows: Vec<AnyElement>) -> impl Iterator<Item = Stateful<Div>> {
        self.ruled(rows.into_iter().map(|row| (0., row)))
    }

    fn ruled(
        &self,
        rows: impl ExactSizeIterator<Item = (f32, AnyElement)>,
    ) -> impl Iterator<Item = Stateful<Div>> {
        let (inset, hair) = (self.density.inset, self.hair);
        let last = rows.len().saturating_sub(1);
        rows.enumerate().map(move |(at, (indent, row))| {
            div()
                .id(at)
                .relative()
                .pl(px(indent))
                .child(row)
                .when(at < last, |row| {
                    row.child(
                        div()
                            .absolute()
                            .left(px(inset + indent))
                            .right(px(inset))
                            .bottom_0()
                            .h(px(hair))
                            .bg(system_color(SystemColor::Separator)),
                    )
                })
        })
    }
}

pub(crate) fn page(
    window: &Window,
    content: Vec<AnyElement>,
    bottom: f32,
    fit: impl Fn(&Window, f32) + 'static,
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
                            .pb(px(bottom))
                            .flex()
                            .flex_col()
                            .children(content)
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        fit(window, titlebar + f32::from(bounds.size.height));
                                    },
                                )
                                .absolute()
                                .size_full(),
                            ),
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

pub(crate) struct Row {
    content: AnyElement,
    tall: f32,
    indent: f32,
}

impl Row {
    pub(crate) fn new(content: impl IntoElement) -> Self {
        Row {
            content: content.into_any_element(),
            tall: COMPACT.row,
            indent: 0.,
        }
    }

    pub(crate) fn tall(mut self, tall: f32) -> Self {
        self.tall = tall;
        self
    }

    pub(crate) fn nested(mut self) -> Self {
        self.indent = COMPACT.indent;
        self
    }
}

pub(crate) struct Sheet {
    form: Form,
    parts: Vec<AnyElement>,
    tall: f32,
}

impl Sheet {
    pub(crate) fn new(width: f32, hair: f32) -> Self {
        Sheet {
            form: Form::at(COMPACT, width, hair),
            parts: Vec::new(),
            tall: PANEL_TITLEBAR + COMPACT.margin,
        }
    }

    pub(crate) fn card(&mut self, id: &'static str, title: Option<&str>, rows: Vec<Row>) {
        let above = match (self.parts.is_empty(), title) {
            (true, _) => COMPACT.margin,
            (false, Some(_)) => COMPACT.section,
            (false, None) => COMPACT.stack,
        };
        let rows_tall: f32 = rows.iter().map(|row| row.tall).sum();
        let title_tall = COMPACT.line + COMPACT.stack;
        self.tall += above + title.map_or(0., |_| title_tall) + rows_tall;
        let rows = rows.into_iter().map(|row| {
            let content = div().h(px(row.tall)).child(row.content);
            (row.indent, content.into_any_element())
        });
        let card = self.form.shell("card").children(self.form.ruled(rows));
        self.parts.push(
            div()
                .id(id)
                .pt(px(above))
                .flex()
                .flex_col()
                .gap(px(COMPACT.stack))
                .when_some(title, |part, title| part.child(COMPACT.heading(title)))
                .child(card)
                .into_any_element(),
        );
    }

    pub(crate) fn button(&mut self, button: impl IntoElement, tall: f32) {
        self.tall += COMPACT.margin + tall;
        self.parts.push(
            div()
                .pt(px(COMPACT.margin))
                .px(px(COMPACT.margin))
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
        page(window, self.parts, COMPACT.margin, fit_panel)
    }
}

fn fit_panel(window: &Window, tall: f32) {
    if let Some(ns) = ns_window(window) {
        resize_later(ns, move |ns| panel_frame(ns, tall));
    }
}

/// Resized inside gpui's update, the window keeps drawing at its old size.
pub(crate) fn resize_later(
    ns: Retained<NSWindow>,
    frame: impl Fn(&NSWindow) -> Option<NSRect> + 'static,
) {
    if frame(&ns).is_none() {
        return;
    }
    let block = RcBlock::new(move || {
        if let Some(fitted) = frame(&ns) {
            ns.setFrame_display(fitted, true);
        }
    });
    unsafe { NSRunLoop::mainRunLoop().performBlock(&block) };
}

/// Runs from a task: the open panel's modal loop would re-enter GPUI's borrow.
pub(crate) fn choose_file<V: 'static>(
    cx: &mut Context<V>,
    from: Option<&'static str>,
    chosen: impl FnOnce(&mut V, PathBuf, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        crate::chrome::toolbar::system_corners_of(&panel);
        if let Some(from) = from {
            panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(from))));
        }
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(path) = panel.URL().and_then(|url| url.path()) else {
            return;
        };
        let path = PathBuf::from(path.to_string());
        this.update(cx, |view, cx| chosen(view, path, cx)).ok();
    })
    .detach();
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
