use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use finder_fs::words;
use gpui::{App, Entity, SharedString};
use objc2::msg_send;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL, ns_string};

use crate::app::FinderDemo;
use crate::data::{Face, Row, byte_string, count, volume_at};
use crate::openwith::url;
use crate::trim::Time;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: &NSURL, options: *const NSDictionary) -> *mut AnyObject;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: &AnyObject,
        index: usize,
        options: *const NSDictionary,
    ) -> *mut NSDictionary;
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

const HELD: usize = 4096;

enum Fact {
    Count,
    Pixels,
    Duration,
    Space,
}

type Key = (SharedString, u64);

thread_local! {
    static INFO: RefCell<HashMap<Key, Option<SharedString>>> = RefCell::new(HashMap::new());
    static WANTED: RefCell<HashMap<Key, Fact>> = RefCell::new(HashMap::new());
}

fn fact(row: &Row) -> Option<Fact> {
    match row.face {
        Face::Folder => Some(Fact::Count),
        Face::Image => Some(Fact::Pixels),
        Face::Movie | Face::Audio => Some(Fact::Duration),
        Face::Volume => Some(Fact::Space),
        _ => None,
    }
}

pub(crate) fn item_info(row: &Row) -> Option<SharedString> {
    let fact = fact(row)?;
    let key = (row.path.clone(), row.modified_at.to_bits());
    if let Some(known) = INFO.with_borrow(|info| info.get(&key).cloned()) {
        return known;
    }
    WANTED.with_borrow_mut(|wanted| wanted.insert(key, fact));
    None
}

pub(crate) fn fetch_wanted(demo: &Entity<FinderDemo>, cx: &mut App) {
    let wanted = WANTED.take();
    if wanted.is_empty() {
        return;
    }
    INFO.with_borrow_mut(|info| {
        if info.len() + wanted.len() > HELD {
            info.clear();
        }
        info.extend(wanted.keys().map(|key| (key.clone(), None)));
    });
    let demo = demo.downgrade();
    cx.spawn(async move |cx| {
        let found: Vec<(Key, Option<SharedString>)> = cx
            .background_executor()
            .spawn(async move {
                wanted
                    .into_iter()
                    .map(|(key, fact)| {
                        let line = autoreleasepool(|_| read(fact, Path::new(key.0.as_ref())));
                        (key, line)
                    })
                    .collect()
            })
            .await;
        demo.update(cx, |_, cx| {
            let any = found.iter().any(|(_, line)| line.is_some());
            INFO.with_borrow_mut(|info| info.extend(found));
            if any {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

fn read(fact: Fact, path: &Path) -> Option<SharedString> {
    let line = match fact {
        Fact::Count => {
            let listed = finder_fs::list::list_dir(path).ok()?;
            let shown = listed.iter().filter(|entry| !entry.hidden).count();
            words::fill(
                &words::plural("U_COUNT", shown, "^0 items"),
                &[&count(shown)],
            )
        }
        Fact::Pixels => {
            let (width, height) = pixels(path)?;
            words::fill(
                &words::finder("IN_P1", "^0\u{200a}×\u{200a}^1"),
                &[&width.to_string(), &height.to_string()],
            )
        }
        Fact::Duration => return crate::preview::duration(seconds(path)?),
        Fact::Space => {
            let volume = volume_at(path.to_str()?)?;
            words::fill(
                &words::finder("IN9_1", "^0, ^1 free"),
                &[
                    &byte_string(volume.capacity),
                    &byte_string(volume.available),
                ],
            )
        }
    };
    Some(line.into())
}

fn pixels(path: &Path) -> Option<(u64, u64)> {
    let url = url(path)?;
    let source = unsafe { Retained::from_raw(CGImageSourceCreateWithURL(&url, std::ptr::null()))? };
    let properties = unsafe {
        Retained::from_raw(CGImageSourceCopyPropertiesAtIndex(
            &source,
            0,
            std::ptr::null(),
        ))?
    };
    let side = |key: &NSString| {
        let side = properties.objectForKey(key)?.downcast::<NSNumber>().ok()?;
        Some(side.unsignedLongLongValue())
    };
    Some((
        side(ns_string!("PixelWidth"))?,
        side(ns_string!("PixelHeight"))?,
    ))
}

fn seconds(path: &Path) -> Option<f64> {
    let url = url(path)?;
    let assets = AnyClass::get(c"AVURLAsset")?;
    let asset: Option<Retained<AnyObject>> = unsafe {
        msg_send![assets, URLAssetWithURL: &*url, options: std::ptr::null::<NSDictionary>()]
    };
    let time: Time = unsafe { msg_send![&*asset?, duration] };
    time.seconds()
}
