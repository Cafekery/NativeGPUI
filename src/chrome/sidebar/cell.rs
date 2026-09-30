use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use gpui::{Pixels, SharedString};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSBundleImageExtension, NSButton, NSCellImagePosition, NSColor, NSEvent, NSFont,
    NSFontTextStyleBody, NSGraphicsContext, NSImage, NSImageAlignment, NSImageScaling,
    NSImageSymbolConfiguration, NSImageSymbolScale, NSImageView, NSLineBreakMode, NSOutlineView,
    NSTableCellView, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSUserInterfaceItemIdentification, NSView, NSWindingRule,
};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
};

use super::model::Node;
use super::{BADGE_GAP, CORE_TYPES_BUNDLE, Shared, SidebarIcon, SidebarRowSize};

pub(super) struct CellIvars {
    group: bool,
    pub(super) badge: Retained<NSTextField>,
    eject: Retained<EjectButton>,
    tint: RefCell<Option<Retained<NSColor>>>,
    muted: Cell<bool>,
}

impl CellIvars {
    fn trailing(&self) -> Option<&NSView> {
        let eject: &NSView = &self.eject;
        let badge: &NSView = &self.badge;
        [eject, badge].into_iter().find(|view| !view.isHidden())
    }
}

define_class!(
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarCell"]
    #[ivars = CellIvars]
    pub(super) struct SidebarCell;

    impl SidebarCell {
        #[unsafe(method(layout))]
        fn layout(&self) {
            let _: () = unsafe { msg_send![super(self), layout] };

            // AppKit restyles a group row's label on display, so this reapplies after every layout.
            if self.ivars().group {
                if let Some(title) = unsafe { self.textField() } {
                    let frame = title.frame();
                    title.setFrameOrigin(NSPoint::new(
                        frame.origin.x,
                        frame.origin.y - GROUP_BASELINE_DROP,
                    ));
                    title.setTextColor(Some(&NSColor::tertiaryLabelColor()));
                }
            }

            let Some(trailing) = self.ivars().trailing() else {
                return;
            };
            let bounds = self.bounds();
            let size = trailing.fittingSize();
            let (x, reserved) = if self.ivars().eject.isHidden() {
                (bounds.size.width - size.width, size.width + BADGE_GAP)
            } else {
                // Finder's title yields the eject's slot and one point more, at 2x only.
                let gap = match self.window() {
                    Some(window) if window.backingScaleFactor() == 2. => 1.,
                    _ => 0.,
                };
                let reserved = EJECT_SLOT + gap;
                (
                    bounds.size.width - reserved + ((EJECT_SLOT - size.width) / 2.).floor(),
                    reserved,
                )
            };
            trailing.setFrame(NSRect::new(
                NSPoint::new(x, ((bounds.size.height - size.height) / 2.).round()),
                size,
            ));

            if let Some(title) = unsafe { self.textField() } {
                let frame = title.frame();
                let width = bounds.size.width - reserved - frame.origin.x;
                title.setFrame(NSRect::new(
                    frame.origin,
                    NSSize::new(width.max(0.), frame.size.height),
                ));
            }
        }
    }
);

struct EjectIvars {
    rest: RefCell<Option<Retained<NSColor>>>,
}

define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarEjectButton"]
    #[ivars = EjectIvars]
    struct EjectButton;

    impl EjectButton {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.retint();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.retint();
        }
    }
);

impl EjectButton {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(EjectIvars {
            rest: RefCell::new(None),
        });
        let eject: Retained<Self> = unsafe { msg_send![super(this), init] };
        eject.setBordered(false);
        eject.setImagePosition(NSCellImagePosition::ImageOnly);
        eject.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("eject.fill"),
                Some(&NSString::from_str("Eject")),
            )
            .and_then(|image| {
                image.imageWithSymbolConfiguration(
                    &NSImageSymbolConfiguration::configurationWithTextStyle_scale(
                        unsafe { NSFontTextStyleBody },
                        NSImageSymbolScale::Small,
                    ),
                )
            })
            .as_deref(),
        );
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveInKeyWindow,
                Some(eject.as_ref()),
                None,
            )
        };
        eject.addTrackingArea(&area);
        eject
    }

    fn rest(&self, ink: Retained<NSColor>) {
        *self.ivars().rest.borrow_mut() = Some(ink);
        self.retint();
    }

    fn retint(&self) {
        let hovered = self.window().is_some_and(|window| {
            window.isKeyWindow()
                && self.mouse_inRect(
                    self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None),
                    self.bounds(),
                )
        });
        let ink = if hovered {
            Some(NSColor::secondaryLabelColor())
        } else {
            self.ivars().rest.borrow().clone()
        };
        self.setContentTintColor(ink.as_deref());
    }
}

// One identifier per row shape: a recycled cell keeps its subviews and `group`.
const ID_GROUP: &str = "NGSidebarGroupRow";
const ID_ICON: &str = "NGSidebarIconRow";
const ID_PLAIN: &str = "NGSidebarPlainRow";

const EJECT_SLOT: f64 = 18.0;
const TAG_DOT_DIAMETER: f64 = 12.5;
const TAG_DOT_LINE: f64 = 1.0;
const TAG_DOTS_OVERLAP: f64 = 4.5;
const GROUP_BASELINE_DROP: f64 = 2.0;
const INACTIVE_LABEL_ALPHA: f64 = 0.47;

fn label(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

fn srgb((r, g, b): (u8, u8, u8)) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        r as f64 / 255.,
        g as f64 / 255.,
        b as f64 / 255.,
        1.,
    )
}

fn make_cell(mtm: MainThreadMarker, identifier: &str, target: &AnyObject) -> Retained<SidebarCell> {
    let badge = label(mtm);
    badge.setAlignment(NSTextAlignment::Right);
    badge.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    badge.setTextColor(Some(&NSColor::secondaryLabelColor()));
    badge.setHidden(true);

    let eject = EjectButton::new(mtm);
    eject.setHidden(true);
    unsafe {
        eject.setTarget(Some(target));
        eject.setAction(Some(sel!(ngSidebarEject:)));
    }

    let this = SidebarCell::alloc(mtm).set_ivars(CellIvars {
        group: identifier == ID_GROUP,
        badge,
        eject,
        tint: RefCell::new(None),
        muted: Cell::new(false),
    });
    let cell: Retained<SidebarCell> = unsafe { msg_send![super(this), init] };
    cell.setIdentifier(Some(&NSString::from_str(identifier)));

    let title = label(mtm);
    cell.addSubview(&title);
    unsafe { cell.setTextField(Some(&title)) };

    if identifier == ID_ICON {
        let icon = NSImageView::new(mtm);
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon.setImageAlignment(NSImageAlignment::AlignCenter);
        cell.addSubview(&icon);
        unsafe { cell.setImageView(Some(&icon)) };
    }

    cell.addSubview(&cell.ivars().badge);
    cell.addSubview(&cell.ivars().eject);
    cell
}

type Dots = HashMap<(Option<(u8, u8, u8)>, bool), Retained<NSImage>>;

type Templates = HashMap<(SharedString, u32), Option<Retained<NSImage>>>;

fn core_types_template(name: &SharedString, points: f64) -> Option<Retained<NSImage>> {
    thread_local! {
        static TEMPLATES: RefCell<Templates> = RefCell::new(HashMap::new());
    }
    let key = (name.clone(), points as u32);
    if let Some(hit) = TEMPLATES.with_borrow(|cache| cache.get(&key).cloned()) {
        return hit;
    }
    let built = build_core_types_template(name, points);
    TEMPLATES.with_borrow_mut(|cache| cache.insert(key, built.clone()));
    built
}

fn nearest_rep_points(available: impl IntoIterator<Item = f64>, wanted: f64) -> Option<f64> {
    available.into_iter().min_by(|a, b| {
        (a - wanted)
            .abs()
            .total_cmp(&(b - wanted).abs())
            .then(b.total_cmp(a))
    })
}

const PRIVATE_SYMBOLS: &str =
    "/System/Library/PrivateFrameworks/SFSymbols.framework/Resources/CoreGlyphsPrivate.bundle";

/// Finder's AirDrop and Applications glyphs are symbols Apple keeps out of the public catalog.
fn system_symbol(name: &str) -> Option<Retained<NSImage>> {
    let name = NSString::from_str(name);
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&name, None).or_else(|| {
        let catalog = NSBundle::bundleWithPath(&NSString::from_str(PRIVATE_SYMBOLS))?;
        NSImage::imageWithSymbolName_bundle_variableValue(&name, Some(&catalog), 1.)
    })
}

pub fn system_template(resource: &str, points: Pixels) -> Option<Retained<NSImage>> {
    let image = build_core_types_template(resource, points.to_f64())?;
    image.setSize(NSSize::new(points.to_f64(), points.to_f64()));
    Some(image)
}

/// The drawing nearest `points`, at its own size: Finder draws its 18pt art in a 17pt row.
fn build_core_types_template(name: &str, points: f64) -> Option<Retained<NSImage>> {
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(CORE_TYPES_BUNDLE))?;
    let source = bundle.imageForResource(&NSString::from_str(name))?;
    let reps = source.representations();
    let chosen = nearest_rep_points(reps.iter().map(|rep| rep.size().width), points)?;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(chosen, chosen));
    // The finest rep of one point size only: the bundle's 1x drawing at 18pt is sheared.
    if let Some(finest) = reps
        .iter()
        .filter(|rep| rep.size().width == chosen)
        .max_by_key(|rep| rep.pixelsWide())
    {
        image.addRepresentation(&finest);
    }
    image.setTemplate(true);
    Some(image)
}

fn ring_path(at: NSRect) -> Retained<NSBezierPath> {
    let ring = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(
            at.origin.x + TAG_DOT_LINE / 2.,
            at.origin.y + TAG_DOT_LINE / 2.,
        ),
        NSSize::new(at.size.width - TAG_DOT_LINE, at.size.height - TAG_DOT_LINE),
    ));
    ring.setLineWidth(TAG_DOT_LINE);
    ring
}

fn tag_dot_image(icon: &SidebarIcon) -> Option<Retained<NSImage>> {
    let color = match icon {
        SidebarIcon::Dot(color) => *color,
        SidebarIcon::Dots => None,
        _ => return None,
    };
    let pair = matches!(icon, SidebarIcon::Dots);
    thread_local! {
        static DOTS: RefCell<Dots> = RefCell::new(HashMap::new());
    }
    if let Some(hit) = DOTS.with_borrow(|cache| cache.get(&(color, pair)).cloned()) {
        return Some(hit);
    }
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        let disc = NSRect::new(rect.origin, NSSize::new(TAG_DOT_DIAMETER, TAG_DOT_DIAMETER));
        match color {
            Some(rgb) => {
                srgb(rgb).setFill();
                NSBezierPath::bezierPathWithOvalInRect(disc).fill();
            }
            None => {
                NSColor::blackColor().setStroke();
                if pair {
                    let behind = NSRect::new(
                        NSPoint::new(disc.origin.x + TAG_DOTS_OVERLAP, disc.origin.y),
                        disc.size,
                    );
                    let clip = NSBezierPath::bezierPathWithRect(rect);
                    clip.appendBezierPathWithOvalInRect(disc);
                    clip.setWindingRule(NSWindingRule::EvenOdd);
                    NSGraphicsContext::saveGraphicsState_class();
                    clip.addClip();
                    ring_path(behind).stroke();
                    NSGraphicsContext::restoreGraphicsState_class();
                }
                ring_path(disc).stroke();
            }
        }
        Bool::YES
    });
    let width = TAG_DOT_DIAMETER + if pair { TAG_DOTS_OVERLAP } else { 0. };
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(width, TAG_DOT_DIAMETER),
        false,
        &draw,
    );
    image.setTemplate(color.is_none());
    DOTS.with_borrow_mut(|cache| cache.insert((color, pair), image.clone()));
    Some(image)
}

fn label_fades_itself() -> bool {
    static SELF_FADE: OnceLock<bool> = OnceLock::new();
    *SELF_FADE.get_or_init(|| {
        NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
            < 27
    })
}

fn tint_cell(cell: &SidebarCell, selected: bool, key: bool) {
    let ivars = cell.ivars();
    // A label colour on a vibrant image view draws far darker than the same colour as text.
    let (icon_ink, eject_ink) = if key {
        (
            Some(NSColor::systemGrayColor()),
            NSColor::tertiaryLabelColor(),
        )
    } else {
        (None, NSColor::quaternaryLabelColor())
    };
    if let Some(icon) = unsafe { cell.imageView() } {
        icon.setContentTintColor(icon_ink.filter(|_| ivars.muted.get()).as_deref());
    }
    ivars.eject.rest(eject_ink);
    let Some(title) = (unsafe { cell.textField() }) else {
        return;
    };
    let fade = label_fades_itself() && !key && !ivars.group;
    let tint = if selected {
        ivars.tint.borrow().clone()
    } else {
        None
    };
    let colour = match tint {
        Some(tag) => tag,
        // An explicit colour loses the accent AppKit substitutes on a selected row.
        None if fade && selected => NSColor::controlAccentColor(),
        None => NSColor::labelColor(),
    };
    let colour = if fade {
        colour.colorWithAlphaComponent(INACTIVE_LABEL_ALPHA)
    } else {
        colour
    };
    title.setTextColor(Some(&colour));
}

pub(super) fn tint_rows(outline: &NSOutlineView) {
    let key = key_window(outline);
    for row in 0..outline.numberOfRows() {
        let Some(view) = outline.viewAtColumn_row_makeIfNecessary(0, row, false) else {
            continue;
        };
        if view.isKindOfClass(SidebarCell::class()) {
            let cell = unsafe { Retained::cast_unchecked::<SidebarCell>(view) };
            tint_cell(&cell, outline.isRowSelected(row), key);
        }
    }
}

fn key_window(view: &NSOutlineView) -> bool {
    view.window().is_some_and(|window| window.isKeyWindow())
}

fn configure_cell(cell: &SidebarCell, node: &Node, icon_points: f64) {
    if let Some(title) = unsafe { cell.textField() } {
        let wanted = NSString::from_str(&node.title);
        if title.stringValue() != wanted {
            title.setStringValue(&wanted);
        }
    }

    *cell.ivars().tint.borrow_mut() = match node.icon {
        Some(SidebarIcon::Dot(Some(rgb))) => Some(srgb(rgb)),
        _ => None,
    };
    cell.ivars().muted.set(node.muted);

    if let Some(view) = unsafe { cell.imageView() } {
        let image = match &node.icon {
            Some(SidebarIcon::Symbol(name)) => system_symbol(name),
            Some(SidebarIcon::SystemResource(name)) => core_types_template(name, icon_points),
            Some(icon @ (SidebarIcon::Dot(_) | SidebarIcon::Dots)) => tag_dot_image(icon),
            None => None,
        };
        view.setImage(image.as_deref());
    }

    let badge = &cell.ivars().badge;
    match node.badge {
        Some(count) => {
            let text = NSString::from_str(&count.to_string());
            if badge.stringValue() != text {
                badge.setStringValue(&text);
            }
            badge.setHidden(false);
        }
        None => badge.setHidden(true),
    }
    cell.ivars().eject.setHidden(!node.eject);

    cell.setNeedsLayout(true);
}

pub(super) fn row_view(
    shared: &Shared,
    outline: &NSOutlineView,
    item: &AnyObject,
    target: &AnyObject,
) -> Option<Retained<NSView>> {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let model = shared.model.borrow();
    let node = model.node(Shared::key_of_item(item))?;

    let identifier = if node.group {
        ID_GROUP
    } else if node.icon.is_some() {
        ID_ICON
    } else {
        ID_PLAIN
    };
    let name = NSString::from_str(identifier);

    let cell = match unsafe { outline.makeViewWithIdentifier_owner(&name, Some(outline)) } {
        Some(view) => unsafe { Retained::cast_unchecked::<SidebarCell>(view) },
        None => make_cell(mtm, identifier, target),
    };
    configure_cell(
        &cell,
        node,
        SidebarRowSize::icon_points(outline.effectiveRowSizeStyle()),
    );
    let row = unsafe { outline.rowForItem(Some(item)) };
    tint_cell(&cell, outline.isRowSelected(row), key_window(outline));
    Some(Retained::into_super(Retained::into_super(cell)))
}

#[cfg(test)]
mod tests {
    use objc2_app_kit::NSTableViewRowSizeStyle;
    use objc2_foundation::NSSize;

    use super::*;
    use crate::chrome::sidebar::SidebarRowSize;

    const AIRDROP_REP_POINTS: [f64; 8] = [16., 16., 18., 18., 24., 24., 32., 32.];

    #[test]
    fn a_medium_icon_box_resolves_to_the_three_arc_airdrop_rep() {
        assert_eq!(
            nearest_rep_points(
                AIRDROP_REP_POINTS,
                SidebarRowSize::icon_points(NSTableViewRowSizeStyle::Medium)
            ),
            Some(18.)
        );
        assert_eq!(
            nearest_rep_points(
                AIRDROP_REP_POINTS,
                SidebarRowSize::icon_points(NSTableViewRowSizeStyle::Small)
            ),
            Some(16.)
        );
        assert_eq!(nearest_rep_points(AIRDROP_REP_POINTS, 30.), Some(32.));
        assert_eq!(nearest_rep_points([], 19.), None);
    }

    #[test]
    fn the_core_types_bundle_yields_a_template_at_its_drawings_size() {
        let image = build_core_types_template("SidebarAirDrop", 17.)
            .expect("CoreTypes.bundle is world-readable and ships SidebarAirDrop");
        assert!(image.isTemplate());
        assert_eq!(image.size(), NSSize::new(18., 18.));
        let reps: Vec<(f64, isize)> = image
            .representations()
            .iter()
            .map(|rep| (rep.size().width, rep.pixelsWide()))
            .collect();
        assert_eq!(reps, vec![(18., 36)], "one point size, at its finest");
        let sized = super::system_template("SidebarAirDrop", gpui::px(17.)).expect("the same art");
        assert_eq!(sized.size(), NSSize::new(17., 17.));
        assert!(build_core_types_template("SidebarNoSuchThing", 17.).is_none());
    }

    #[test]
    fn finders_own_glyphs_load_as_symbols_by_name() {
        for name in ["airdrop", "appstore"] {
            assert!(system_symbol(name).is_some(), "{name}");
        }
        assert!(system_symbol("not.a.real.sf.symbol").is_none());
    }
}
