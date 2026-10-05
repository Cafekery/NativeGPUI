use std::cell::OnceCell;
use std::ptr;

use gpui::{AnyElement, Context, Div, FontWeight, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor, system_color};
use native_gpui::{ControlSize, NativeLabel, native_image, native_label, native_popup};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSBitmapFormat, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage, NSImageInterpolation,
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
const PHOTO_GAP: f32 = 8.;
const LINE_GAP: f32 = 2.;
const POPUP_W: f32 = 114.;
const FLOOR: f32 = 12.;
const RULE_Y: f32 = 2. * INSET + PHOTO as f32;
const FOOTER_RULE: f32 = 61.;
const HAIRLINE: f32 = 0.5;
const PHOTO_NAME: &str = "ng.finder.airdrop.photo";
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

fn round(photo: &NSImage) -> Option<Retained<NSImage>> {
    let pixels = (PHOTO * PHOTO_SCALE) as isize;
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
    let points = NSSize::new(PHOTO, PHOTO);
    rep.setSize(points);
    let size = photo.size();
    let side = size.width.min(size.height);
    let square = NSRect::new(
        NSPoint::new((size.width - side) / 2., (size.height - side) / 2.),
        NSSize::new(side, side),
    );
    let rect = NSRect::new(NSPoint::ZERO, points);
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    context.setImageInterpolation(NSImageInterpolation::High);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    NSBezierPath::bezierPathWithOvalInRect(rect).addClip();
    photo.drawInRect_fromRect_operation_fraction(
        rect,
        square,
        NSCompositingOperation::SourceOver,
        1.,
    );
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
                let photo = round(&*account_photo()?)?;
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

fn secondary(id: &'static str, text: &str, size: f32) -> NativeLabel {
    native_label(id, text)
        .font_size(px(size))
        .color(SystemColor::SecondaryLabel)
}

fn label_width(text: &str, size: f32) -> f32 {
    let width = tokens::appkit_string_width(
        tokens::system_font_family(),
        px(size),
        FontWeight::NORMAL,
        text,
    );
    width.unwrap_or_default().ceil() + 2. * LABEL_INSET
}

impl FinderDemo {
    pub(crate) fn airdrop_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = discoverable();
        let this = cx.entity().downgrade();
        let popup = native_popup("ng.finder.airdrop.mode", mode)
            .items(MODES.map(|(_, title)| title))
            .control_size(ControlSize::Small)
            .width(px(POPUP_W))
            .on_change(move |picked, _, cx| {
                if *picked != mode {
                    cx.open_url(SETTINGS);
                }
                this.update(cx, |_, cx| cx.notify()).ok();
            });
        let allow = "Allow me to be discovered by: ";
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
                        row.child(native_image(PHOTO_NAME, PHOTO_NAME))
                    })
                    .child(
                        div()
                            .flex_1()
                            .ml(px(-LABEL_INSET))
                            .flex()
                            .flex_col()
                            .child(secondary("ng.finder.airdrop.as", "Use AirDrop as", 13.))
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
                    .child(secondary(
                        "ng.finder.airdrop.about",
                        "AirDrop lets you share instantly with people nearby.",
                        12.,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().w(px(label_width(allow, 12.))).child(secondary(
                                "ng.finder.airdrop.allow",
                                allow,
                                12.,
                            )))
                            .child(popup),
                    ),
            )
            .into_any_element()
    }
}
