use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;

use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDate, NSNumber, NSString, ns_string};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDateGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemCreate(allocator: *const c_void, path: *const c_void) -> *mut c_void;
    fn MDItemCopyAttribute(item: *mut c_void, name: *const c_void) -> *mut c_void;
    fn MDItemCopyAttributeNames(item: *mut c_void) -> *mut c_void;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub content_created: Option<f64>,
    pub last_used: Option<f64>,
    pub pixels: Option<(u64, u64)>,
    pub color_space: Option<String>,
    pub color_profile: Option<String>,
    pub version: Option<String>,
    pub pages: Option<u64>,
    pub page: Option<(u64, u64)>,
    pub security: Option<String>,
    pub encoders: Vec<String>,
    pub category: Option<String>,
    pub duration: Option<f64>,
    /// Every attribute by Spotlight name, as text.
    pub values: Vec<(String, String)>,
}

/// An owned CoreFoundation reference.
struct Cf(NonNull<c_void>);

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

impl Cf {
    fn new(created: *mut c_void) -> Option<Self> {
        NonNull::new(created).map(Self)
    }

    fn item(path: &Path) -> Option<Self> {
        let path = NSString::from_str(path.to_str()?);
        Self::new(unsafe { MDItemCreate(std::ptr::null(), (&*path as *const NSString).cast()) })
    }

    fn attribute(&self, name: &str) -> Option<Self> {
        let key = NSString::from_str(name);
        Self::new(unsafe {
            MDItemCopyAttribute(self.0.as_ptr(), (&*key as *const NSString).cast())
        })
    }

    fn is(&self, id: usize) -> bool {
        unsafe { CFGetTypeID(self.0.as_ptr()) == id }
    }

    fn cast<T>(&self) -> &T {
        unsafe { self.0.cast::<T>().as_ref() }
    }

    fn string(&self) -> Option<String> {
        if !self.is(unsafe { CFStringGetTypeID() }) {
            return None;
        }
        let text = self.cast::<NSString>().to_string();
        (!text.is_empty()).then_some(text)
    }

    fn number(&self) -> Option<f64> {
        self.is(unsafe { CFNumberGetTypeID() })
            .then(|| self.cast::<NSNumber>().doubleValue())
    }

    fn date(&self) -> Option<f64> {
        self.is(unsafe { CFDateGetTypeID() })
            .then(|| self.cast::<NSDate>().timeIntervalSinceReferenceDate())
    }

    fn text(&self) -> Option<String> {
        if self.is(unsafe { CFBooleanGetTypeID() }) {
            let on = self.cast::<NSNumber>().boolValue();
            return Some(if on { "Yes" } else { "No" }.to_owned());
        }
        if let Some(number) = self.number() {
            return Some(match number.fract() == 0. {
                true => format!("{number:.0}"),
                false => format!("{number:.2}").trim_end_matches('0').to_owned(),
            });
        }
        if !self.is(unsafe { CFArrayGetTypeID() }) {
            return None;
        }
        let words: Vec<String> = self
            .cast::<NSArray<AnyObject>>()
            .iter()
            .filter_map(|word| Some(word.downcast::<NSString>().ok()?.to_string()))
            .collect();
        (!words.is_empty()).then(|| words.join(", "))
    }

    fn strings(&self) -> Vec<String> {
        if !self.is(unsafe { CFArrayGetTypeID() }) {
            return Vec::new();
        }
        self.cast::<NSArray<NSString>>()
            .iter()
            .map(|text| text.to_string())
            .collect()
    }
}

fn pair(item: &Cf, width: &str, height: &str) -> Option<(u64, u64)> {
    let width = item.attribute(width)?.number()?;
    let height = item.attribute(height)?.number()?;
    (width > 0. && height > 0.).then_some((width as u64, height as u64))
}

/// Spotlight makes this up from the modification date where it has not indexed the item.
pub fn last_used(path: &Path) -> Option<f64> {
    Cf::item(path)?.attribute("kMDItemLastUsedDate")?.date()
}

pub fn bundle_version(path: &Path) -> Option<String> {
    ["Contents/Info.plist", "Resources/Info.plist", "Info.plist"]
        .into_iter()
        .find_map(|inner| {
            let version = crate::plist_dict(&std::fs::read(path.join(inner)).ok()?)?
                .objectForKey(ns_string!("CFBundleShortVersionString"))?
                .downcast::<NSString>()
                .ok()?
                .to_string();
            (!version.is_empty()).then_some(version)
        })
}

pub fn meta(path: &Path) -> Meta {
    let Some(item) = Cf::item(path) else {
        return Meta::default();
    };
    let string = |name| item.attribute(name)?.string();
    let number = |name| item.attribute(name)?.number();
    let date = |name| item.attribute(name)?.date();
    Meta {
        content_created: date("kMDItemContentCreationDate"),
        last_used: date("kMDItemLastUsedDate"),
        pixels: pair(&item, "kMDItemPixelWidth", "kMDItemPixelHeight"),
        color_space: string("kMDItemColorSpace"),
        color_profile: string("kMDItemProfileName"),
        version: string("kMDItemVersion"),
        pages: number("kMDItemNumberOfPages").map(|pages| pages as u64),
        page: pair(&item, "kMDItemPageWidth", "kMDItemPageHeight"),
        security: string("kMDItemSecurityMethod"),
        encoders: item
            .attribute("kMDItemEncodingApplications")
            .map(|value| value.strings())
            .unwrap_or_default(),
        category: string("kMDItemAppStoreCategory"),
        duration: number("kMDItemDurationSeconds"),
        values: values(&item),
    }
}

fn values(item: &Cf) -> Vec<(String, String)> {
    let Some(names) = Cf::new(unsafe { MDItemCopyAttributeNames(item.0.as_ptr()) }) else {
        return Vec::new();
    };
    names
        .strings()
        .into_iter()
        .filter_map(|name| {
            let value = item.attribute(&name)?;
            let text = value.string().or_else(|| value.text())?;
            Some((name, text))
        })
        .collect()
}
