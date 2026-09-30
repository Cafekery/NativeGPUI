use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use finder_fs::tags::Tag;
use gpui::{Context, Window, px, size};
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::tokens::Appearance;
use native_gpui::{NativePopover, PopoverBehavior, PopoverOptions, present_popover};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAttachmentAttributeName, NSAttributedStringAttachmentConveniences,
    NSBezierPath, NSButton, NSCell, NSCellImagePosition, NSColor, NSControlTextEditingDelegate,
    NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSMutableParagraphStyle,
    NSParagraphStyleAttributeName, NSStringDrawing, NSTextAlignment, NSTextAttachment,
    NSTextAttachmentCell, NSTextField, NSTextFieldDelegate, NSTextView, NSView,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString,
    NSNotification, NSNumber, NSObject, NSPoint, NSRange, NSRect, NSSize, NSString,
};

use crate::app::FinderDemo;
use crate::tags::{all, tag_ns_color};

const WIDTH: f64 = 252.;
const ROW: f64 = 22.;
const ROWS: usize = 12;
const HEAD: f64 = 74.;
const TOKEN_H: f64 = 15.;
const TOKEN_PAD: f64 = 10.;
const TOKEN_TEXT: f64 = 11.;
const TOKEN_RADIUS: f64 = 2.;

pub(crate) struct TagEditor {
    popover: NativePopover,
    field: Retained<NSTextField>,
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

fn token_attributes(color: &NSColor) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    let font = NSFont::systemFontOfSize(TOKEN_TEXT);
    NSDictionary::from_slices(
        &[
            unsafe { NSFontAttributeName },
            unsafe { NSForegroundColorAttributeName },
            unsafe { NSParagraphStyleAttributeName },
        ],
        &[font.as_ref(), color.as_ref(), style.as_ref()],
    )
}

define_class!(
    #[unsafe(super(NSTextAttachmentCell, NSCell, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGTagToken"]
    struct Token;

    impl Token {
        #[unsafe(method(cellSize))]
        fn cell_size(&self) -> NSSize {
            let attributes = token_attributes(&NSColor::controlTextColor());
            let text = unsafe { self.stringValue().sizeWithAttributes(Some(&attributes)) };
            NSSize::new((text.width + TOKEN_PAD).ceil(), TOKEN_H)
        }

        #[unsafe(method(cellBaselineOffset))]
        fn cell_baseline_offset(&self) -> NSPoint {
            NSPoint::new(0., -3.)
        }

        #[unsafe(method(drawWithFrame:inView:))]
        fn draw_with_frame(&self, frame: NSRect, view: Option<&NSView>) {
            self.draw(frame, view);
        }
    }
);

impl Token {
    fn new(tag: &Tag, mtm: MainThreadMarker) -> Retained<Self> {
        let name = NSString::from_str(&tag.name);
        let token: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initTextCell: &*name] };
        unsafe { token.setRepresentedObject(Some(&NSNumber::new_u8(tag.color))) };
        token
    }

    fn color(&self) -> u8 {
        self.representedObject()
            .and_then(|color| color.downcast::<NSNumber>().ok())
            .map_or(0, |color| color.as_u8())
    }

    fn tag(&self) -> Tag {
        Tag {
            name: self.stringValue().to_string(),
            color: self.color(),
        }
    }

    /// TextKit 2 draws attachments in its own views, inside the text view holding the selection.
    fn selected(&self, view: Option<&NSView>) -> bool {
        let mut view = view.map(|view| view.retain());
        while let Some(at) = view {
            if let Some(text) = at.downcast_ref::<NSTextView>() {
                let Some(storage) = (unsafe { text.textStorage() }) else {
                    return false;
                };
                let range = text.selectedRange();
                return (range.location..range.location + range.length).any(|index| {
                    unsafe {
                        storage.attribute_atIndex_effectiveRange(
                            NSAttachmentAttributeName,
                            index,
                            std::ptr::null_mut(),
                        )
                    }
                    .and_then(|found| found.downcast::<NSTextAttachment>().ok())
                    .and_then(|attachment| attachment.attachmentCell())
                    .is_some_and(|cell| {
                        let cell: &AnyObject = cell.as_ref();
                        std::ptr::addr_eq(cell, self)
                    })
                });
            }
            view = unsafe { at.superview() };
        }
        false
    }

    fn draw(&self, frame: NSRect, view: Option<&NSView>) {
        let flipped = view.is_some_and(|view| view.isFlipped());
        let pill = NSRect::new(
            NSPoint::new(frame.origin.x + 1., frame.origin.y + f64::from(flipped)),
            NSSize::new(frame.size.width - 2., frame.size.height - 1.),
        );
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            pill,
            TOKEN_RADIUS,
            TOKEN_RADIUS,
        );
        let dark = Appearance::of(&NSAppearance::currentDrawingAppearance()) == Appearance::Dark;
        let color = tag_ns_color(self.color());
        match &color {
            Some(color) => color.colorWithAlphaComponent(if dark { 0.6 } else { 0.25 }),
            None => NSColor::quaternaryLabelColor(),
        }
        .setFill();
        path.fill();
        if let Some(color) = color {
            color
                .colorWithAlphaComponent(if dark { 0.6 } else { 0.5 })
                .setFill();
            path.fill();
        }
        let text = match self.selected(view) {
            true => NSColor::alternateSelectedControlTextColor(),
            false => NSColor::controlTextColor(),
        };
        let lower = if flipped { 1. } else { -1. };
        let rect = NSRect::new(
            NSPoint::new(frame.origin.x, frame.origin.y + lower),
            frame.size,
        );
        unsafe {
            self.stringValue()
                .drawInRect_withAttributes(rect, Some(&token_attributes(&text)))
        };
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGTagFieldDelegate"]
    struct Tokeniser;

    unsafe impl NSObjectProtocol for Tokeniser {}

    unsafe impl NSControlTextEditingDelegate for Tokeniser {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(object) = notification.object() else {
                return;
            };
            let Some(field) = object.downcast_ref::<NSTextField>() else {
                return;
            };
            if read(field).1.iter().any(|typed| typed.contains(',')) {
                show(field, &tags_in(field));
            }
        }
    }

    unsafe impl NSTextFieldDelegate for Tokeniser {}
);

fn tokeniser(mtm: MainThreadMarker) -> Retained<Tokeniser> {
    thread_local! {
        static DELEGATE: RefCell<Option<Retained<Tokeniser>>> = const { RefCell::new(None) };
    }
    DELEGATE.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| unsafe { msg_send![Tokeniser::alloc(mtm), init] })
            .clone()
    })
}

fn editor(field: &NSTextField) -> Option<Retained<NSTextView>> {
    field.currentEditor()?.downcast::<NSTextView>().ok()
}

/// The field's tokens, and the text typed between them.
fn read(field: &NSTextField) -> (Vec<Tag>, Vec<String>) {
    let text = match editor(field).and_then(|editor| unsafe { editor.textStorage() }) {
        Some(storage) => Retained::into_super(Retained::into_super(storage)),
        None => field.attributedStringValue(),
    };
    let string = text.string();
    let (mut tokens, mut typed) = (Vec::new(), Vec::new());
    let mut at = 0;
    while at < string.length() {
        let mut run = NSRange::new(at, 1);
        let found = unsafe {
            text.attribute_atIndex_effectiveRange(NSAttachmentAttributeName, at, &mut run)
        };
        let token = found
            .as_deref()
            .and_then(|found| found.downcast_ref::<NSTextAttachment>())
            .and_then(|attachment| attachment.attachmentCell())
            .and_then(|cell| {
                let cell: &AnyObject = cell.as_ref();
                cell.downcast_ref::<Token>().map(Token::tag)
            });
        match token {
            Some(token) => tokens.push(token),
            None => typed.push(string.substringWithRange(run).to_string()),
        }
        at = run.location + run.length.max(1);
    }
    (tokens, typed)
}

/// The field's tags: its tokens, then each typed name, as the tag of that name if one is listed.
pub(crate) fn tags_in(field: &NSTextField) -> Vec<Tag> {
    let (tokens, typed) = read(field);
    let typed = typed
        .iter()
        .flat_map(|run| run.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(tag_named);
    let mut tags: Vec<Tag> = Vec::new();
    for tag in tokens.into_iter().chain(typed) {
        if !tags.iter().any(|held| held.name == tag.name) {
            tags.push(tag);
        }
    }
    tags
}

fn show(field: &NSTextField, tags: &[Tag]) {
    let mtm = MainThreadMarker::from(field);
    let text = NSMutableAttributedString::new();
    for tag in tags {
        let token = Token::new(tag, mtm);
        let cell: &NSTextAttachmentCell = &token;
        let attachment = NSTextAttachment::new();
        attachment.setAttachmentCell(Some(ProtocolObject::from_ref(cell)));
        text.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(
            &attachment,
        ));
    }
    let font = NSFont::systemFontOfSize(NSFont::systemFontSize());
    unsafe {
        text.addAttribute_value_range(
            NSFontAttributeName,
            font.as_ref(),
            NSRange::new(0, text.length()),
        )
    };
    let Some(editor) = editor(field) else {
        field.setAttributedStringValue(&text);
        return;
    };
    let whole = NSRange::new(0, editor.string().length());
    if editor.shouldChangeTextInRange_replacementString(whole, Some(&text.string())) {
        if let Some(storage) = unsafe { editor.textStorage() } {
            storage.replaceCharactersInRange_withAttributedString(whole, &text);
        }
        editor.didChangeText();
    }
    editor.setSelectedRange(NSRange::new(text.length(), 0));
}

/// Finder's tag field: each tag a token in its colour; typed names become tokens at a comma.
pub(crate) fn tag_field(
    mtm: MainThreadMarker,
    frame: NSRect,
    tags: &[Tag],
) -> Retained<NSTextField> {
    let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);
    field.setAllowsEditingTextAttributes(true);
    field.setImportsGraphics(true);
    if let Some(cell) = field.cell() {
        cell.setWraps(true);
        cell.setScrollable(false);
    }
    field.setFont(Some(&NSFont::systemFontOfSize(NSFont::systemFontSize())));
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*tokeniser(mtm)))) };
    show(&field, tags);
    field
}

fn tag_named(name: &str) -> Tag {
    let lower = name.to_lowercase();
    all()
        .iter()
        .find(|tag| tag.name.to_lowercase() == lower)
        .cloned()
        .unwrap_or(Tag {
            name: name.to_string(),
            color: 0,
        })
}

/// The item's tags, each listed one in the sidebar's colour.
pub(crate) fn held_tags(path: &Path) -> Vec<Tag> {
    finder_fs::tags::of(path)
        .into_iter()
        .map(|tag| Tag {
            color: all()
                .iter()
                .find(|listed| listed.name == tag.name)
                .map_or(tag.color, |listed| listed.color),
            name: tag.name,
        })
        .collect()
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
        let mut held = held_tags(first);
        for path in &paths[1..] {
            let also = finder_fs::tags::of(path);
            held.retain(|tag| also.iter().any(|other| other.name == tag.name));
        }
        let shared: Vec<String> = held.iter().map(|tag| tag.name.clone()).collect();
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
        let built: Rc<RefCell<Option<Retained<NSTextField>>>> = Rc::default();
        let slot = built.clone();
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

            let field = tag_field(
                mtm,
                NSRect::new(
                    NSPoint::new(10., height - 64.),
                    NSSize::new(WIDTH - 20., 26.),
                ),
                &held,
            );
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
                let (field, tag, pick) = (field.clone(), tag.clone(), demo.clone());
                actions.on_action(&button, move |_, app| {
                    let mut now = tags_in(&field);
                    if !now.iter().any(|held| held.name == tag.name) {
                        now.push(tag.clone());
                        show(&field, &now);
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
        let now = tags_in(&editor.field);
        let names: Vec<String> = now.iter().map(|tag| tag.name.clone()).collect();
        let before = std::mem::replace(&mut editor.shared, names.clone());
        let add: Vec<Tag> = now
            .into_iter()
            .filter(|tag| !before.contains(&tag.name))
            .collect();
        let remove: Vec<String> = before
            .into_iter()
            .filter(|name| !names.contains(name))
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
