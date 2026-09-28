use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use block2::RcBlock;
use finder_fs::tags::Tag;
use gpui::{Context, Window, px, size};
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{NativePopover, PopoverBehavior, PopoverOptions, present_popover};
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSBezierPath, NSButton, NSCellImagePosition, NSColor, NSImage, NSTextAlignment, NSTextField,
    NSTokenField, NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRange, NSRect, NSSize, NSString};

use crate::app::FinderDemo;
use crate::tags::{all, tag_ns_color};

const WIDTH: f64 = 252.;
const ROW: f64 = 22.;
const ROWS: usize = 12;
const HEAD: f64 = 74.;

pub(crate) struct TagEditor {
    popover: NativePopover,
    field: Retained<NSTokenField>,
    shared: Vec<String>,
    paths: Vec<PathBuf>,
}

fn dot(color: Option<Retained<NSColor>>) -> Retained<NSImage> {
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let circle = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(1.5, 1.5),
            NSSize::new(9., 9.),
        ));
        match &color {
            Some(color) => {
                color.setFill();
                circle.fill();
            }
            None => {
                NSColor::secondaryLabelColor().setStroke();
                circle.setLineWidth(1.);
                circle.stroke();
            }
        }
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(12., 12.), true, &handler)
}

fn names(field: &NSTokenField) -> Vec<String> {
    field
        .objectValue()
        .and_then(|value| value.downcast::<NSArray>().ok())
        .map(|array| {
            array
                .iter()
                .filter_map(|item| item.downcast::<NSString>().ok())
                .map(|name| name.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn set_names(field: &NSTokenField, names: &[String]) {
    let names: Vec<Retained<NSString>> =
        names.iter().map(|name| NSString::from_str(name)).collect();
    unsafe { field.setObjectValue(Some(NSArray::from_retained_slice(&names).as_ref())) };
}

fn tag_named(name: &str) -> Tag {
    all()
        .iter()
        .find(|tag| tag.name == name)
        .cloned()
        .unwrap_or(Tag {
            name: name.to_string(),
            color: 0,
        })
}

impl FinderDemo {
    pub(crate) fn edit_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tag_editor.is_some() {
            self.commit_tags(cx);
            if let Some(editor) = self.tag_editor.take() {
                editor.popover.close();
            }
            return;
        }
        let paths = self.paths();
        let Some(first) = paths.first() else {
            return;
        };
        let mut shared: Vec<String> = finder_fs::tags::of(first)
            .into_iter()
            .map(|tag| tag.name)
            .collect();
        for path in &paths[1..] {
            let held = finder_fs::tags::of(path);
            shared.retain(|name| held.iter().any(|tag| &tag.name == name));
        }
        let title = match paths.len() {
            1 => format!(
                "Assign tags to \u{201c}{}\u{201d}",
                first
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            ),
            count => format!("Assign tags to {count} items"),
        };
        let listed = all().len().min(ROWS);
        let height = HEAD + listed as f64 * ROW + 10.;
        let spec = PopoverOptions {
            size: size(px(WIDTH as f32), px(height as f32)),
            edge: SurfaceEdge::Below,
            behavior: PopoverBehavior::Transient,
            animates: true,
        };
        let demo = cx.entity().downgrade();
        let built: Rc<RefCell<Option<Retained<NSTokenField>>>> = Rc::default();
        let slot = built.clone();
        let tokens = shared.clone();
        let popover = present_popover(window, cx, &self.ql_icon, spec, move |mtm, actions| {
            let view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
            );
            let label = NSTextField::labelWithString(&NSString::from_str(&title), mtm);
            label.setFrame(NSRect::new(
                NSPoint::new(12., height - 30.),
                NSSize::new(WIDTH - 24., 18.),
            ));
            label.setAlignment(NSTextAlignment::Center);
            view.addSubview(&label);

            let field = NSTokenField::initWithFrame(
                NSTokenField::alloc(mtm),
                NSRect::new(
                    NSPoint::new(10., height - 64.),
                    NSSize::new(WIDTH - 20., 26.),
                ),
            );
            set_names(&field, &tokens);
            view.addSubview(&field);
            slot.replace(Some(field.clone()));

            let on_return = demo.clone();
            actions.on_action(&field, move |_, app| {
                on_return.update(app, |this, cx| this.commit_tags(cx)).ok();
            });

            for (at, tag) in all().iter().take(ROWS).enumerate() {
                let y = height - HEAD - (at as f64 + 1.) * ROW;
                let button = NSButton::initWithFrame(
                    NSButton::alloc(mtm),
                    NSRect::new(NSPoint::new(12., y), NSSize::new(WIDTH - 24., ROW)),
                );
                button.setTitle(&NSString::from_str(&tag.name));
                button.setBordered(false);
                button.setImage(Some(&dot(tag_ns_color(tag.color))));
                button.setImagePosition(NSCellImagePosition::ImageLeft);
                button.setAlignment(NSTextAlignment::Left);
                let (field, name, pick) = (field.clone(), tag.name.clone(), demo.clone());
                actions.on_action(&button, move |_, app| {
                    let mut now = names(&field);
                    if !now.contains(&name) {
                        now.push(name.clone());
                        set_names(&field, &now);
                    }
                    pick.update(app, |this, cx| this.commit_tags(cx)).ok();
                });
                view.addSubview(&button);
            }
            view
        });
        let (Some(popover), Some(field)) = (popover, built.take()) else {
            return;
        };
        if let Some(window) = field.window() {
            window.makeFirstResponder(Some(&field));
            if let Some(editor) = field.currentEditor() {
                editor.setSelectedRange(NSRange::new(editor.string().length(), 0));
            }
        }
        let closed = cx.entity().downgrade();
        let popover = popover.on_close(window, cx, move |_, app| {
            closed
                .update(app, |this, cx| {
                    this.commit_tags(cx);
                    this.tag_editor = None;
                    cx.notify();
                })
                .ok();
        });
        self.tag_editor = Some(TagEditor {
            popover,
            field,
            shared,
            paths,
        });
    }

    /// Writes only the change since the last write, so a tag not every item shares stays put.
    fn commit_tags(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.tag_editor else {
            return;
        };
        let now = names(&editor.field);
        let before = std::mem::replace(&mut editor.shared, now.clone());
        let add: Vec<Tag> = now
            .iter()
            .filter(|name| !before.contains(name))
            .map(|name| tag_named(name))
            .collect();
        let remove: Vec<String> = before
            .into_iter()
            .filter(|name| !now.contains(name))
            .collect();
        if add.is_empty() && remove.is_empty() {
            return;
        }
        let paths = editor.paths.clone();
        self.retag(
            paths,
            move |paths| {
                finder_fs::tags::apply(paths, &add, &remove).ok();
            },
            cx,
        );
    }
}
