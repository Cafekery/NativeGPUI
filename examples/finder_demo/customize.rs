use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::RcBlock;
use finder_fs::folder_icon::{self, Decoration};
use finder_fs::tags::Tag;
use finder_fs::words::{customize as said, symbol_label};
use gpui::{App, Context, SharedString, Window, point, px, size};
use native_gpui::surfaces::SurfaceEdge;
use native_gpui::{
    NativePopover, PopoverBehavior, PopoverOptions, SurfaceActions, present_popover,
};
use objc2::MainThreadOnly;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{Bool, ProtocolObject};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityHeadingRole, NSAccessibilityMenuButtonRole, NSApplication,
    NSBezelStyle, NSBezierPath, NSButton, NSCellImagePosition, NSColor,
    NSControlTextDidChangeNotification, NSFocusRingType, NSFont, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSLineBreakMode, NSMenuItem, NSPopUpArrowPosition,
    NSPopUpButton, NSPopUpButtonCell, NSScrollView, NSTextAlignment, NSTextField, NSView,
    NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString,
};

use crate::app::FinderDemo;
use crate::art::Anchor;
use crate::data::Line;
use crate::tags::{tag_ns_color, tags};

const WIDTH: f64 = 320.;
const HEIGHT: f64 = 670.;
const SWATCHES: usize = 7;
const GRID_TOP: f64 = 203.;
const GRID_H: f64 = 426.;
const CELL: f64 = 44.;
const ACROSS: usize = 7;
const HEADING_H: f64 = 16.;
const GAP: f64 = 10.;

#[rustfmt::skip]
const GROUPS: [(&str, &str, &[&str]); 4] = [
    ("People", "people", &[
        "person.fill", "person.2.fill", "person.crop.circle", "figure.stand", "figure.stand.dress",
        "figure.pregnant", "figure.arms.open", "figure.2.arms.open",
        "figure.2.and.child.holdinghands", "figure.and.child.holdinghands", "figure", "figure.2",
        "figure.child", "face.smiling", "brain", "brain.head.profile.fill", "eye",
        "eye.half.closed", "eyes", "eyebrow", "nose.fill", "mustache.fill", "mouth.fill",
        "ear.fill", "hearingdevice.ear.fill", "lungs.fill", "hand.raised.fill",
        "hand.raised.palm.facing.fill", "hand.raised.fingers.spread.fill", "hand.thumbsup.fill",
        "hand.thumbsdown.fill", "hand.point.up.left.fill", "hand.tap.fill", "hand.rays.fill",
        "rectangle.and.hand.point.up.left.fill", "hand.point.left.fill", "hand.point.right.fill",
        "hand.point.up.fill", "hand.point.up.braille.fill", "hand.point.down.fill",
        "hand.wave.fill",
    ]),
    ("Animals & Nature", "animals_and_nature", &[
        "hare.fill", "tortoise.fill", "dog.fill", "cat.fill", "lizard.fill", "bird.fill",
        "ant.fill", "ladybug.fill", "fish.fill", "pawprint.fill", "leaf.fill",
        "globe.americas.fill", "globe.europe.africa.fill", "globe.asia.australia.fill",
        "globe.central.south.asia.fill", "sun.max.fill", "sunrise.fill", "sunset.fill",
        "sun.horizon.fill", "moon.fill", "sparkles", "moon.stars.fill", "cloud.fill",
        "cloud.drizzle.fill", "cloud.rain.fill", "cloud.snow.fill", "cloud.bolt.fill",
        "cloud.sun.fill", "cloud.moon.fill", "wind", "snowflake", "tornado", "tropicalstorm",
        "hurricane", "thermometer.medium", "rainbow", "drop.fill", "water.waves", "flame.fill",
        "mountain.2.fill", "tree.fill", "camera.macro", "allergens.fill", "fossil.shell.fill",
        "atom",
    ]),
    ("Food & Drinks", "food_and_drink", &[
        "fork.knife", "spoon.serving", "cup.and.saucer.fill", "cup.and.heat.waves.fill", "mug.fill",
        "takeoutbag.and.cup.and.straw.fill", "wineglass.fill", "waterbottle.fill",
        "apple.slice.fill", "carrot.fill", "frying.pan.fill", "popcorn.fill", "birthday.cake.fill",
    ]),
    ("Activity", "activity", &[
        "soccerball", "baseball", "basketball.fill", "american.football.fill",
        "american.football.professional.fill", "rugbyball.fill", "tennis.racket",
        "hockey.puck.fill", "cricket.ball.fill", "tennisball.fill", "volleyball",
        "sportscourt.fill", "athletic.track", "skateboard.fill",
    ]),
];

pub(crate) struct Customizer {
    key: SharedString,
    name: SharedString,
    decoration: Option<Decoration>,
    held: Vec<String>,
    shown: Option<(NativePopover, Parts)>,
}

struct Parts {
    icon: Retained<NSImageView>,
    symbols: Vec<(&'static str, Retained<NSButton>)>,
    swatches: Vec<(&'static Tag, Retained<NSButton>)>,
    more: Retained<NSPopUpButton>,
    more_tags: Vec<&'static Tag>,
    field: Retained<NSTextField>,
    typing: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Drop for Parts {
    fn drop(&mut self) {
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.typing.as_ref()) };
    }
}

enum Change {
    Decorate(Option<Decoration>),
    Toggle(&'static Tag),
    Untag,
    More,
    Emoji,
    Typed,
}

fn frame(x: f64, top: f64, width: f64, height: f64, within: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(x, within - top - height),
        NSSize::new(width, height),
    )
}

fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

fn symbol(name: &str, points: f64, color: Option<&NSColor>) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None)?;
    let mut look = NSImageSymbolConfiguration::configurationWithPointSize_weight(points, 0.);
    if let Some(color) = color {
        look = look.configurationByApplyingConfiguration(
            &NSImageSymbolConfiguration::configurationWithHierarchicalColor(color),
        );
    }
    image.imageWithSymbolConfiguration(&look)
}

fn disc(color: Option<Retained<NSColor>>, on: bool) -> Retained<NSImage> {
    let draw = RcBlock::new(move |_: NSRect| -> Bool {
        let circle = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(4., 4.),
            NSSize::new(18., 18.),
        ));
        match &color {
            Some(color) => {
                color.setFill();
                circle.fill();
            }
            None => {
                NSColor::secondaryLabelColor().setStroke();
                circle.stroke();
            }
        }
        if on {
            let ring = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                NSPoint::new(1.5, 1.5),
                NSSize::new(23., 23.),
            ));
            ring.setLineWidth(1.5);
            NSColor::labelColor().setStroke();
            ring.stroke();
        }
        Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(26., 26.), true, &draw)
}

fn identify(view: &NSView, id: &str) {
    view.setAccessibilityIdentifier(Some(&ns(id)));
}

fn image_button(mtm: MainThreadMarker, at: NSRect) -> Retained<NSButton> {
    let button = NSButton::initWithFrame(NSButton::alloc(mtm), at);
    button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
    button.setShowsBorderOnlyWhileMouseInside(true);
    button.setTitle(&ns(""));
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    button.setImageScaling(NSImageScaling::ScaleNone);
    button
}

fn push_button(mtm: MainThreadMarker, title: &str, at: NSRect) -> Retained<NSButton> {
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    button.setFrame(at);
    button
}

fn bold_label(mtm: MainThreadMarker, text: &str, at: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&ns(text), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.)));
    label.setFrame(at);
    label
}

impl FinderDemo {
    pub(crate) fn tags_or_customize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.chosen_lines().as_slice() {
            [line] if line.row.opens.is_some() => self.customize_folder(line, window, cx),
            _ => self.edit_tags(window, cx),
        }
    }

    fn customize_folder(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        self.close_customizer();
        let folder = Path::new(line.key.as_ref());
        self.customizer = Some(Customizer {
            key: line.key.clone(),
            name: line.row.display.clone(),
            decoration: folder_icon::read(folder),
            held: finder_fs::tags::of(folder)
                .into_iter()
                .map(|tag| tag.name)
                .collect(),
            shown: None,
        });
        // A fresh anchor gets bounds only if the next paint shows the folder's icon.
        self.ql_icon = Anchor::default();
        cx.notify();
        let demo = cx.entity().downgrade();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                demo.update(cx, |this, cx| this.show_customizer(window, cx))
                    .ok();
            })
        });
    }

    fn close_customizer(&mut self) {
        if let Some((popover, _)) = self.customizer.take().and_then(|open| open.shown) {
            popover.close();
        }
    }

    fn show_customizer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(customizer) = &self.customizer else {
            return;
        };
        let middle = window.viewport_size();
        let anchor = self.icon_anchor_or(point(middle.width / 2., middle.height / 2.));
        let spec = PopoverOptions {
            size: size(px(WIDTH as f32), px(HEIGHT as f32)),
            edge: SurfaceEdge::Right,
            behavior: PopoverBehavior::Transient,
            animates: true,
        };
        let demo = cx.entity().downgrade();
        let change = move |change: Change| {
            let demo = demo.clone();
            move |window: &mut Window, app: &mut App| {
                demo.update(app, |this, cx| this.change_folder(&change, window, cx))
                    .ok();
            }
        };
        let mut built = None;
        let popover = present_popover(window, cx, &anchor, spec, |mtm, actions| {
            let (view, parts) = build(mtm, actions, customizer, &change);
            built = Some(parts);
            view
        });
        let (Some(popover), Some(parts)) = (popover, built) else {
            self.customizer = None;
            return;
        };
        let closed = cx.entity().downgrade();
        let popover = popover.on_close(window, cx, move |_, app| {
            closed
                .update(app, |this, cx| {
                    this.customizer = None;
                    cx.notify();
                })
                .ok();
        });
        if let Some(customizer) = &mut self.customizer {
            customizer.shown = Some((popover, parts));
            customizer.refresh();
        }
    }

    fn change_folder(&mut self, change: &Change, window: &mut Window, cx: &mut Context<Self>) {
        let Some(customizer) = &mut self.customizer else {
            return;
        };
        let Some((_, parts)) = &customizer.shown else {
            return;
        };
        let folder = PathBuf::from(customizer.key.as_ref());
        match change {
            Change::Decorate(decoration) => {
                if let Err(error) = folder_icon::write(&folder, decoration.as_ref()) {
                    return self.complain(&error.to_string());
                }
                customizer.decoration = decoration.clone();
                crate::custom::set_custom_icon(&customizer.key, decoration.is_some());
                for image in crate::icons::redraw(&customizer.key) {
                    cx.drop_image(image, None);
                }
            }
            Change::Toggle(tag) => {
                match customizer.held.iter().position(|name| *name == tag.name) {
                    Some(at) => _ = customizer.held.remove(at),
                    None => customizer.held.push(tag.name.clone()),
                }
                let tag = *tag;
                self.retag(
                    vec![folder],
                    move |paths| _ = finder_fs::tags::toggle(paths, tag),
                    cx,
                );
            }
            Change::Untag => {
                customizer.held.clear();
                self.retag(
                    vec![folder],
                    |paths| _ = finder_fs::tags::set(&paths[0], &[]),
                    cx,
                );
            }
            Change::More => {
                let at = parts.more.indexOfSelectedItem();
                let listed = parts.more_tags.len() as isize;
                let tag = usize::try_from(at - 1)
                    .ok()
                    .and_then(|at| parts.more_tags.get(at).copied());
                match tag {
                    Some(tag) => self.change_folder(&Change::Toggle(tag), window, cx),
                    None if at == listed + 2 => {
                        self.close_customizer();
                        self.edit_tags(window, cx);
                    }
                    None if at == listed + 3 => {
                        self.close_customizer();
                        crate::settings::open_pane("Tags", cx);
                    }
                    None => {}
                }
                return;
            }
            Change::Emoji => {
                if let Some(popover) = parts.field.window() {
                    parts.field.setEditable(true);
                    popover.makeFirstResponder(Some(&parts.field));
                    NSApplication::sharedApplication(parts.field.mtm())
                        .orderFrontCharacterPalette(None);
                }
                return;
            }
            Change::Typed => {
                let typed = parts.field.stringValue().to_string();
                parts.field.setStringValue(&ns(""));
                let emoji = typed.chars().any(|c| !c.is_ascii() && !c.is_alphanumeric())
                    && !typed.chars().any(char::is_whitespace);
                if emoji {
                    self.change_folder(
                        &Change::Decorate(Some(Decoration::Emoji(typed))),
                        window,
                        cx,
                    );
                }
                return;
            }
        }
        if let Some(customizer) = &self.customizer {
            customizer.refresh();
        }
        cx.notify();
    }
}

impl Customizer {
    fn refresh(&self) {
        let Some((_, parts)) = &self.shown else {
            return;
        };
        parts.icon.setImage(Some(
            &NSWorkspace::sharedWorkspace().iconForFile(&ns(&self.key)),
        ));
        let chosen = match &self.decoration {
            Some(Decoration::Symbol(name)) => Some(name.as_str()),
            _ => None,
        };
        for (name, button) in &parts.symbols {
            let tint = match chosen == Some(*name) {
                true => NSColor::controlAccentColor(),
                false => NSColor::secondaryLabelColor(),
            };
            button.setContentTintColor(Some(&tint));
        }
        let held = |tag: &Tag| self.held.contains(&tag.name);
        for (tag, button) in &parts.swatches {
            let on = held(tag);
            button.setImage(Some(&disc(tag_ns_color(tag.color), on)));
            button.setToolTip(Some(&ns(&match on {
                true => said("Remove tag from folder", "Remove tag from folder"),
                false => said("Add tag to folder", "Add tag to folder"),
            })));
        }
        for (at, tag) in parts.more_tags.iter().enumerate() {
            if let Some(item) = parts.more.itemAtIndex(at as isize + 1) {
                item.setState(held(tag).into());
            }
        }
    }
}

fn build<Handler: Fn(&mut Window, &mut App) + 'static>(
    mtm: MainThreadMarker,
    actions: &SurfaceActions,
    customizer: &Customizer,
    change: &impl Fn(Change) -> Handler,
) -> (Retained<NSView>, Parts) {
    let at = |x, top, width, height| frame(x, top, width, height, HEIGHT);
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, HEIGHT)),
    );

    let icon = NSImageView::initWithFrame(NSImageView::alloc(mtm), at(112., 12., 96., 96.));
    icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    view.addSubview(&icon);
    let name = bold_label(mtm, &customizer.name, at(10., 116., WIDTH - 20., 16.));
    name.setAlignment(NSTextAlignment::Center);
    name.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    identify(&name, "folderName");
    view.addSubview(&name);

    let untag = image_button(mtm, at(11., 147., 20., 20.));
    untag.setImage(symbol("circle.slash", 15., None).as_deref());
    untag.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    untag.setAccessibilityLabel(Some(&ns(&said(
        "Remove all tags [text]",
        "Remove all tags",
    ))));
    identify(&untag, "RemoveAllTagsButton");
    actions.on_action(&untag, change(Change::Untag));
    view.addSubview(&untag);

    let swatches: Vec<_> = tags()
        .iter()
        .take(SWATCHES)
        .enumerate()
        .map(|(index, tag)| {
            let swatch = image_button(mtm, at(41. + 34. * index as f64, 144., 26., 26.));
            swatch.setAccessibilityLabel(Some(&ns(&tag.name)));
            identify(&swatch, &format!("tag-{}", tag.name));
            actions.on_action(&swatch, change(Change::Toggle(tag)));
            view.addSubview(&swatch);
            (tag, swatch)
        })
        .collect();

    let more_tags: Vec<_> = crate::tags::all()
        .iter()
        .filter(|tag| !swatches.iter().any(|(shown, _)| shown.name == tag.name))
        .collect();
    let more = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        at(279., 143., 28., 28.),
        true,
    );
    more.setBordered(false);
    if let Some(cell) = more
        .cell()
        .and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
    {
        cell.setArrowPosition(NSPopUpArrowPosition::NoArrow);
    }
    let add = |title: &str| {
        more.addItemWithTitle(&ns(title));
        more.lastItem()
    };
    if let Some(face) = add("") {
        face.setImage(symbol("plus", 13., None).as_deref());
    }
    for tag in &more_tags {
        let (glyph, tint) = match tag_ns_color(tag.color) {
            Some(color) => ("circle.fill", color),
            None => ("circle", NSColor::secondaryLabelColor()),
        };
        if let Some(item) = add(&tag.name) {
            item.setImage(symbol(glyph, 13., Some(&tint)).as_deref());
        }
    }
    if let Some(menu) = more.menu() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    add(&said("Edit Tags…", "Edit Tags…"));
    add(&said("Edit All Tags…", "Edit All Tags…"));
    more.setAccessibilityRole(Some(unsafe { NSAccessibilityMenuButtonRole }));
    more.setAccessibilityTitle(Some(&ns("Add")));
    identify(&more, "tagOverflowMenuButton");
    actions.on_action(&more, change(Change::More));
    view.addSubview(&more);

    let rows = |count: usize| count.div_ceil(ACROSS) as f64 * CELL;
    let inside = GROUPS.iter().fold(GAP, |top, (_, _, list)| {
        top + HEADING_H + rows(list.len()) + GAP
    });
    let grid = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, inside)),
    );
    grid.setAccessibilityLabel(Some(&ns(&said("Symbol Selection", "Symbol Selection"))));
    let pick = ns(&said(
        "select symbol for folder",
        "select symbol for folder",
    ));
    let mut symbols = Vec::new();
    let mut top = GAP;
    for (english, group, list) in GROUPS {
        let title = said(&format!("{english} [Symbol category group name]"), english);
        let heading = bold_label(mtm, &title, frame(10., top, WIDTH - 20., HEADING_H, inside));
        heading.setAccessibilityRole(Some(unsafe { NSAccessibilityHeadingRole }));
        heading.setAccessibilityLabel(Some(&ns(&title)));
        identify(&heading, &format!("symbolGroupHeader-group-key-{group}"));
        grid.addSubview(&heading);
        top += HEADING_H;
        for (index, name) in list.iter().enumerate() {
            let x = 6. + (index % ACROSS) as f64 * CELL;
            let y = top + (index / ACROSS) as f64 * CELL;
            let cell = image_button(mtm, frame(x, y, CELL, CELL, inside));
            cell.setImage(symbol(name, 20., None).as_deref());
            if let Some(text) = symbol_label(name) {
                cell.setAccessibilityLabel(Some(&ns(&text)));
            }
            identify(&cell, name);
            cell.setToolTip(Some(&pick));
            let decoration = Decoration::Symbol((*name).to_owned());
            actions.on_action(&cell, change(Change::Decorate(Some(decoration))));
            grid.addSubview(&cell);
            symbols.push((*name, cell));
        }
        top += rows(list.len()) + GAP;
    }
    let scroll =
        NSScrollView::initWithFrame(NSScrollView::alloc(mtm), at(0., GRID_TOP, WIDTH, GRID_H));
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(&grid));
    let clip = scroll.contentView();
    clip.scrollToPoint(NSPoint::new(0., inside - GRID_H));
    scroll.reflectScrolledClipView(&clip);
    identify(&scroll, "symbolScrollView");
    view.addSubview(&scroll);

    let clear_title = said("Clear folder symbol or emoji [button text]", "Clear");
    let clear = push_button(mtm, &clear_title, at(8., 638., 147., 24.));
    identify(&clear, "clearButton");
    actions.on_action(&clear, change(Change::Decorate(None)));
    view.addSubview(&clear);
    let emoji_title = said("Open Emoji picker [button text]", "Emoji");
    let emoji = push_button(mtm, &emoji_title, at(165., 638., 147., 24.));
    let hint = said(
        "Select emoji for folder [ax hint]",
        "Select emoji for folder",
    );
    identify(&emoji, "emojiButton");
    emoji.setToolTip(Some(&ns(&hint)));
    actions.on_action(&emoji, change(Change::Emoji));
    view.addSubview(&emoji);

    // The character palette types into the first responder, which Emoji makes this field.
    let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), at(165., 638., 1., 1.));
    field.setEditable(false);
    field.setAlphaValue(0.);
    field.setFocusRingType(NSFocusRingType::None);
    field.setAccessibilityElement(false);
    actions.on_action(&field, change(Change::Typed));
    view.addSubview(&field);
    // A field sends its action only when editing ends; each insertion posts this instead.
    let weak = Weak::from_retained(&field);
    let changed = RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Some(field) = weak.load() {
            unsafe { field.sendAction_to(field.action(), field.target().as_deref()) };
        }
    });
    let typing = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSControlTextDidChangeNotification),
            Some(&field),
            None,
            &changed,
        )
    };

    let parts = Parts {
        icon,
        symbols,
        swatches,
        more,
        more_tags,
        field,
        typing,
    };
    (view, parts)
}
