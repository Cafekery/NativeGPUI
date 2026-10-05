use std::cell::OnceCell;
use std::ptr;

use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px};
use native_gpui::tokens::{SystemColor, system_color};
use native_gpui::{ControlSize, native_image, native_label, native_popup};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSBitmapFormat, NSBitmapImageRep, NSColor, NSCompositingOperation,
    NSDeviceRGBColorSpace, NSFont, NSGraphicsContext, NSImage, NSImageInterpolation, NSShadow,
    NSTextField, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSData, NSFullUserName, NSPoint, NSRect, NSSize, NSString, NSUserDefaults, NSUserName,
    ns_string,
};

use crate::app::FinderDemo;
use crate::chrome::path_bar::LABEL_INSET;
use crate::chrome::surface::toolbar_rule;

const INSET: f32 = 16.;
const PHOTO: f64 = 48.;
const PHOTO_SCALE: f64 = 2.;
const BADGE: f64 = 18.;
const BADGE_ROOM: f64 = 24.;
const BADGE_BLUR: f64 = 21.;
const BADGE_SHADOW: f64 = 0.2;
const BADGE_ICON: &str =
    "/System/Library/PrivateFrameworks/FinderKit.framework/Versions/A/PlugIns/AirDrop.appex";
const PHOTO_GAP: f32 = 8.;
const LINE_GAP: f32 = 2.;
const POPUP_W: f32 = 114.;
const POPUP_FONT: f32 = 11.;
const FOOT_FONT: f32 = 12.;
const BASELINE_DROP: f32 = 2.;
const FLOOR: f32 = 12.;
const RULE_Y: f32 = 2. * INSET + PHOTO as f32;
const FOOTER_RULE: f32 = 61.;
const HAIRLINE: f32 = 0.5;
const PHOTO_NAME: &str = "ng.finder.airdrop.photo";
const ALLOW: &str = "Allow me to be discovered by: ";
const SETTINGS: &str = "x-apple.systempreferences:com.apple.AirDrop-Handoff-Settings.extension";
const MODES: [(&str, &str); 3] = [
    ("Off", "No One"),
    ("Contacts Only", "Contacts Only"),
    ("Everyone", "Everyone"),
];

#[link(name = "OpenDirectory", kind = "framework")]
unsafe extern "C" {
    static kODRecordTypeUsers: &'static NSString;
    static kODAttributeTypeJPEGPhoto: &'static NSString;
    static kODAttributeTypePicture: &'static NSString;
}

thread_local! {
    static PHOTO_SHOWN: OnceCell<Option<Retained<NSImage>>> = const { OnceCell::new() };
    static SHARINGD: Option<Retained<NSUserDefaults>> = NSUserDefaults::initWithSuiteName(
        NSUserDefaults::alloc(),
        Some(ns_string!("com.apple.sharingd")),
    );
}

fn values(record: &AnyObject, attribute: &NSString) -> Option<Retained<NSArray>> {
    unsafe {
        msg_send![record, valuesForAttribute: attribute, error: ptr::null_mut::<*mut AnyObject>()]
    }
}

fn account_photo() -> Option<Retained<NSImage>> {
    let session: *mut AnyObject =
        unsafe { msg_send![AnyClass::get(c"ODSession")?, defaultSession] };
    let node: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            AnyClass::get(c"ODNode")?,
            nodeWithSession: session,
            name: ns_string!("/Local/Default"),
            error: ptr::null_mut::<*mut AnyObject>()
        ]
    };
    let record: Option<Retained<AnyObject>> = unsafe {
        msg_send![
            &*node?,
            recordWithRecordType: kODRecordTypeUsers,
            name: &*NSUserName(),
            attributes: ptr::null::<AnyObject>(),
            error: ptr::null_mut::<*mut AnyObject>()
        ]
    };
    let record = record?;
    let jpeg = values(&record, unsafe { kODAttributeTypeJPEGPhoto })
        .and_then(|found| found.firstObject()?.downcast::<NSData>().ok())
        .and_then(|data| NSImage::initWithData(NSImage::alloc(), &data));
    jpeg.or_else(|| {
        let found = values(&record, unsafe { kODAttributeTypePicture })?;
        let path = found.firstObject()?.downcast::<NSString>().ok()?;
        NSImage::initWithContentsOfFile(NSImage::alloc(), &path)
    })
}

fn badged(photo: &NSImage) -> Option<Retained<NSImage>> {
    let side = PHOTO + BADGE_ROOM;
    let pixels = (side * PHOTO_SCALE) as isize;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            pixels,
            pixels,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::empty(),
            pixels * 4,
            32,
        )
    }?;
    let points = NSSize::new(side, side);
    rep.setSize(points);
    let size = photo.size();
    let crop = size.width.min(size.height);
    let square = NSRect::new(
        NSPoint::new((size.width - crop) / 2., (size.height - crop) / 2.),
        NSSize::new(crop, crop),
    );
    let rect = NSRect::new(NSPoint::new(0., BADGE_ROOM), NSSize::new(PHOTO, PHOTO));
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    context.setImageInterpolation(NSImageInterpolation::High);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    NSGraphicsContext::saveGraphicsState_class();
    NSBezierPath::bezierPathWithOvalInRect(rect).addClip();
    photo.drawInRect_fromRect_operation_fraction(
        rect,
        square,
        NSCompositingOperation::SourceOver,
        1.,
    );
    NSGraphicsContext::restoreGraphicsState_class();
    if std::path::Path::new(BADGE_ICON).exists() {
        let shadow = NSShadow::new();
        shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0., BADGE_SHADOW)));
        shadow.setShadowBlurRadius(BADGE_BLUR);
        shadow.set();
        // The 18 pt rep's squircle is 16 pt, flush with the photo's corner.
        NSWorkspace::sharedWorkspace()
            .iconForFile(ns_string!(BADGE_ICON))
            .drawInRect_fromRect_operation_fraction(
                NSRect::new(
                    NSPoint::new(PHOTO - BADGE + 1., BADGE_ROOM - 1.),
                    NSSize::new(BADGE, BADGE),
                ),
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.,
            );
    }
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let image = NSImage::initWithSize(NSImage::alloc(), points);
    image.addRepresentation(&rep);
    Some(image)
}

fn photo_named() -> bool {
    PHOTO_SHOWN.with(|shown| {
        shown
            .get_or_init(|| {
                let photo = badged(&*account_photo()?)?;
                photo.setName(Some(ns_string!(PHOTO_NAME))).then_some(photo)
            })
            .is_some()
    })
}

fn discoverable() -> usize {
    SHARINGD
        .with(|defaults| {
            defaults
                .as_ref()?
                .stringForKey(ns_string!("DiscoverableMode"))
        })
        .and_then(|mode| {
            let mode = mode.to_string();
            MODES.iter().position(|(kept, _)| mode == *kept)
        })
        .unwrap_or(1)
}

fn rule() -> Div {
    div().absolute().bg(system_color(SystemColor::Separator))
}

fn allow_width() -> f32 {
    native_gpui::fitting_size("NSTextField.airdrop_allow", |mtm| {
        let label = NSTextField::labelWithString(&NSString::from_str(ALLOW), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(FOOT_FONT.into())));
        label.sizeToFit();
        label.frame().size
    })
    .width as f32
}

impl FinderDemo {
    pub(crate) fn airdrop_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = discoverable();
        let this = cx.entity().downgrade();
        let popup = native_popup("ng.finder.airdrop.mode", mode)
            .items(MODES.map(|(_, title)| title))
            .control_size(ControlSize::Small)
            .font_size(px(POPUP_FONT))
            .tinted(true)
            .width(px(POPUP_W))
            .on_change(move |picked, _, cx| {
                if *picked != mode {
                    cx.open_url(SETTINGS);
                }
                this.update(cx, |_, cx| cx.notify()).ok();
            });
        div()
            .size_full()
            .relative()
            .when(!toolbar_rule(), |body| {
                body.child(rule().top_0().left_0().right_0().h(px(HAIRLINE)))
            })
            .child(
                div()
                    .absolute()
                    .top(px(INSET))
                    .left(px(INSET))
                    .right(px(INSET))
                    .flex()
                    .items_center()
                    .gap(px(PHOTO_GAP))
                    .when(photo_named(), |row| {
                        row.child(
                            div()
                                .size(px(PHOTO as f32))
                                .flex_none()
                                .child(native_image(PHOTO_NAME, PHOTO_NAME)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .ml(px(-LABEL_INSET))
                            .flex()
                            .flex_col()
                            .child(
                                native_label("ng.finder.airdrop.as", "Use AirDrop as")
                                    .font_size(px(13.))
                                    .color(SystemColor::SecondaryLabel),
                            )
                            .child(
                                native_label(
                                    "ng.finder.airdrop.name",
                                    &NSFullUserName().to_string(),
                                )
                                .font_size(px(15.))
                                .weight(FontWeight::SEMIBOLD),
                            ),
                    ),
            )
            .child(
                rule()
                    .top(px(RULE_Y))
                    .left(px(INSET))
                    .right(px(INSET))
                    .h(px(1.)),
            )
            .child(rule().bottom(px(FOOTER_RULE)).left_0().right_0().h(px(1.)))
            .child(
                div()
                    .absolute()
                    .left(px(INSET - LABEL_INSET))
                    .right(px(INSET))
                    .bottom(px(FLOOR))
                    .flex()
                    .flex_col()
                    .gap(px(LINE_GAP))
                    .child(
                        native_label(
                            "ng.finder.airdrop.about",
                            "AirDrop lets you share instantly with people nearby.",
                        )
                        .font_size(px(FOOT_FONT)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .child(
                                div().w(px(allow_width())).mt(px(BASELINE_DROP)).child(
                                    native_label("ng.finder.airdrop.allow", ALLOW)
                                        .font_size(px(FOOT_FONT))
                                        .color(SystemColor::ControlAccent),
                                ),
                            )
                            .child(popup),
                    ),
            )
            .into_any_element()
    }
}
