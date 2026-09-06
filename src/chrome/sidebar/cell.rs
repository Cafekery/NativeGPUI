//! The rows the outline view draws: one recycled `NSTableCellView` per shape, and the artwork a
//! row wears.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Pixels, SharedString};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSBundleImageExtension, NSButton, NSCellImagePosition, NSColor, NSFont,
    NSFontWeightRegular, NSGraphicsContext, NSImage, NSImageAlignment, NSImageScaling,
    NSImageSymbolConfiguration, NSImageView, NSLineBreakMode, NSOutlineView, NSTableCellView,
    NSTextAlignment, NSTextField, NSUserInterfaceItemIdentification, NSView, NSWindingRule,
};
use objc2_foundation::{MainThreadMarker, NSBundle, NSPoint, NSRect, NSSize, NSString};

use super::model::Node;
use super::{BADGE_GAP, CORE_TYPES_BUNDLE, Shared, SidebarIcon, SidebarRowSize};

pub(super) struct CellIvars {
    pub(super) group: bool,
    pub(super) badge: RefCell<Option<Retained<NSTextField>>>,
    pub(super) eject: RefCell<Option<Retained<NSButton>>>,
    /// A tag row's own colour: what Finder draws its label in while the row is selected.
    pub(super) tint: RefCell<Option<Retained<NSColor>>>,
}

impl CellIvars {
    fn trailing(&self) -> Option<Retained<NSView>> {
        let visible = |view: &NSView| !view.isHidden();
        if let Some(eject) = self.eject.borrow().clone().filter(|v| visible(v)) {
            return Some(Retained::into_super(Retained::into_super(eject)));
        }
        let badge = self.badge.borrow().clone().filter(|v| visible(v))?;
        Some(Retained::into_super(Retained::into_super(badge)))
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

            // AppKit restyles a group row's label as it displays, so both the drop and the
            // colour have to be re-applied after its own layout, not once at build time.
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

            // Never hide the image view: that collapses AppKit's icon box and shifts the label.
            let Some(trailing) = self.ivars().trailing() else {
                return;
            };
            let bounds = self.bounds();
            let size = trailing.fittingSize();
            trailing.setFrame(NSRect::new(
                NSPoint::new(
                    bounds.size.width - size.width,
                    ((bounds.size.height - size.height) / 2.).round(),
                ),
                size,
            ));

            if let Some(title) = unsafe { self.textField() } {
                let frame = title.frame();
                let width = bounds.size.width - size.width - BADGE_GAP - frame.origin.x;
                title.setFrame(NSRect::new(
                    frame.origin,
                    NSSize::new(width.max(0.), frame.size.height),
                ));
            }
        }
    }
);

// `NSTableView` keeps one recycling queue per identifier, so rows are split by shape.
const ID_GROUP: &str = "NGSidebarGroupRow";
const ID_ICON: &str = "NGSidebarIconRow";
const ID_PLAIN: &str = "NGSidebarPlainRow";

fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

const EJECT_POINTS: f64 = 10.0;
const TAG_DOT_DIAMETER: f64 = 12.5;
const TAG_DOT_LINE: f64 = 1.0;
const TAG_DOTS_OVERLAP: f64 = 4.5;
/// Finder sets its section headers on the row's baseline, where AppKit centres them.
const GROUP_BASELINE_DROP: f64 = 2.0;

fn make_cell(
    mtm: MainThreadMarker,
    identifier: &str,
    with_icon: bool,
    target: &AnyObject,
) -> Retained<SidebarCell> {
    let this = SidebarCell::alloc(mtm).set_ivars(CellIvars {
        group: identifier == ID_GROUP,
        badge: RefCell::new(None),
        eject: RefCell::new(None),
        tint: RefCell::new(None),
    });
    let cell: Retained<SidebarCell> = unsafe { msg_send![super(this), init] };
    cell.setIdentifier(Some(&NSString::from_str(identifier)));

    let title = label(mtm, "");
    cell.addSubview(&title);
    unsafe { cell.setTextField(Some(&title)) };

    if with_icon {
        let icon = NSImageView::new(mtm);
        icon.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        icon.setImageAlignment(NSImageAlignment::AlignCenter);
        cell.addSubview(&icon);
        unsafe { cell.setImageView(Some(&icon)) };
    }

    let badge = label(mtm, "");
    badge.setAlignment(NSTextAlignment::Right);
    badge.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    badge.setTextColor(Some(&NSColor::secondaryLabelColor()));
    badge.setHidden(true);
    cell.addSubview(&badge);
    *cell.ivars().badge.borrow_mut() = Some(badge);

    let eject = NSButton::new(mtm);
    eject.setBordered(false);
    eject.setImagePosition(NSCellImagePosition::ImageOnly);
    eject.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("eject.fill"),
            Some(&NSString::from_str("Eject")),
        )
        .and_then(|image| {
            image.imageWithSymbolConfiguration(
                &NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    EJECT_POINTS,
                    unsafe { NSFontWeightRegular },
                ),
            )
        })
        .as_deref(),
    );
    eject.setContentTintColor(Some(&NSColor::quaternaryLabelColor()));
    eject.setHidden(true);
    unsafe {
        eject.setTarget(Some(target));
        eject.setAction(Some(sel!(ngSidebarEject:)));
    }
    cell.addSubview(&eject);
    *cell.ivars().eject.borrow_mut() = Some(eject);

    cell
}

/// A tag dot, by the colour it is filled with and whether it is the pair every tag wears.
type Dots = HashMap<(Option<(u8, u8, u8)>, bool), Retained<NSImage>>;

/// A CoreTypes template, by name and whole-point size: `None` for a name the bundle lacks.
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
    available.into_iter().fold(None, |held, candidate| {
        let better = held.is_none_or(|held: f64| {
            let (a, b) = ((candidate - wanted).abs(), (held - wanted).abs());
            a < b || (a == b && candidate < held)
        });
        Some(if better { candidate } else { held? })
    })
}

/// A [`CORE_TYPES_BUNDLE`] resource as a template image, at the representation nearest `points`.
pub fn system_template(resource: &str, points: Pixels) -> Option<Retained<NSImage>> {
    build_core_types_template(resource, points.to_f64())
}

fn build_core_types_template(name: &str, points: f64) -> Option<Retained<NSImage>> {
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(CORE_TYPES_BUNDLE))?;
    let source = bundle.imageForResource(&NSString::from_str(name))?;
    let reps = source.representations();
    let chosen = nearest_rep_points(reps.iter().map(|rep| rep.size().width), points)?;
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(chosen, chosen));
    for rep in reps.iter() {
        // One point size only: 16pt@2x and 32pt@1x are both 32px but are different drawings.
        if rep.size().width == chosen {
            image.addRepresentation(&rep);
        }
    }
    // The bundle's images arrive with `isTemplate` false, so nothing would tint them.
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

/// Finder's tag dot: a disc in the tag's own colour when it has one, a template-tinted ring when
/// it does not, a pair of rings for every tag at once. Drawn rather than an SF Symbol, which the
/// cell would baseline-align, not centre.
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
            Some((r, g, b)) => {
                NSColor::colorWithSRGBRed_green_blue_alpha(
                    r as f64 / 255.,
                    g as f64 / 255.,
                    b as f64 / 255.,
                    1.,
                )
                .setFill();
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

/// Finder draws a selected tag row's label in the tag's own colour, never the accent one.
fn tint_cell(cell: &SidebarCell, selected: bool) {
    let Some(title) = (unsafe { cell.textField() }) else {
        return;
    };
    let tint = cell.ivars().tint.borrow().clone().filter(|_| selected);
    title.setTextColor(Some(tint.as_deref().unwrap_or(&NSColor::labelColor())));
}

pub(super) fn tint_selected_rows(outline: &NSOutlineView) {
    for row in 0..outline.numberOfRows() {
        let Some(view) = outline.viewAtColumn_row_makeIfNecessary(0, row, false) else {
            continue;
        };
        if view.isKindOfClass(SidebarCell::class()) {
            let cell = unsafe { Retained::cast_unchecked::<SidebarCell>(view) };
            tint_cell(&cell, outline.isRowSelected(row));
        }
    }
}

fn configure_cell(cell: &SidebarCell, node: &Node, icon_points: f64) {
    if let Some(title) = unsafe { cell.textField() } {
        let wanted = NSString::from_str(&node.title);
        if title.stringValue() != wanted {
            title.setStringValue(&wanted);
        }
    }

    *cell.ivars().tint.borrow_mut() = match node.icon {
        Some(SidebarIcon::Dot(Some((r, g, b)))) => {
            Some(NSColor::colorWithSRGBRed_green_blue_alpha(
                r as f64 / 255.,
                g as f64 / 255.,
                b as f64 / 255.,
                1.,
            ))
        }
        _ => None,
    };

    if let Some(view) = unsafe { cell.imageView() } {
        let image = match &node.icon {
            Some(SidebarIcon::Symbol(name)) => {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(name),
                    None,
                )
            }
            Some(SidebarIcon::SystemResource(name)) => core_types_template(name, icon_points),
            Some(icon @ (SidebarIcon::Dot(_) | SidebarIcon::Dots)) => tag_dot_image(icon),
            None => None,
        };
        view.setImage(image.as_deref());
    }

    if let Some(badge) = cell.ivars().badge.borrow().as_ref() {
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
    }

    if let Some(eject) = cell.ivars().eject.borrow().as_ref() {
        eject.setHidden(!node.eject);
    }

    cell.setNeedsLayout(true);
}

pub(super) fn row_view(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    item: &AnyObject,
    target: &AnyObject,
) -> Option<Retained<NSView>> {
    let mtm = MainThreadMarker::new().expect("outline view callbacks are main-thread");
    let model = shared.model.borrow();
    let node = model.node(Shared::key_of_item(item))?;

    let (identifier, with_icon) = if node.group {
        (ID_GROUP, false)
    } else if node.icon.is_some() {
        (ID_ICON, true)
    } else {
        (ID_PLAIN, false)
    };
    let name = NSString::from_str(identifier);

    let cell = match unsafe { outline.makeViewWithIdentifier_owner(&name, Some(outline)) } {
        Some(view) => unsafe { Retained::cast_unchecked::<SidebarCell>(view) },
        None => make_cell(mtm, identifier, with_icon, target),
    };
    configure_cell(
        &cell,
        node,
        SidebarRowSize::icon_points(outline.effectiveRowSizeStyle()),
    );
    let row = unsafe { outline.rowForItem(Some(item)) };
    tint_cell(&cell, outline.isRowSelected(row));
    Some(unsafe { Retained::cast_unchecked::<NSView>(cell) })
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
    fn the_core_types_bundle_yields_a_sized_template() {
        let image = build_core_types_template("SidebarAirDrop", 19.)
            .expect("CoreTypes.bundle is world-readable and ships SidebarAirDrop");
        assert!(image.isTemplate());
        assert_eq!(image.size(), NSSize::new(18., 18.));
        let points: Vec<f64> = image
            .representations()
            .iter()
            .map(|rep| rep.size().width)
            .collect();
        assert_eq!(points, vec![18., 18.], "both scales of one point size");
        assert!(build_core_types_template("SidebarNoSuchThing", 19.).is_none());
    }
}
