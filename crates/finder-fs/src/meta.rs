//! What Spotlight knows about one file: the attributes Finder's preview pane lists under
//! Information, at a round trip per file.

use std::ffi::c_void;
use std::path::Path;

use objc2_foundation::{NSArray, NSDate, NSNumber, NSString};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDateGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemCreate(allocator: *const c_void, path: *const c_void) -> *mut c_void;
    fn MDItemCopyAttribute(item: *mut c_void, name: *const c_void) -> *mut c_void;
}

/// The attributes the preview card draws, each missing where Spotlight has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    /// When the camera or the encoder made the content, which is not when the file was written.
    pub content_created: Option<f64>,
    pub last_used: Option<f64>,
    pub pixels: Option<(u64, u64)>,
    pub color_space: Option<String>,
    pub color_profile: Option<String>,
    pub version: Option<String>,
    pub pages: Option<u64>,
    /// A page's size in points, which Finder labels Resolution.
    pub page: Option<(u64, u64)>,
    pub security: Option<String>,
    pub encoders: Vec<String>,
    pub category: Option<String>,
    pub duration: Option<f64>,
}

struct Item(*mut c_void);

impl Drop for Item {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

impl Item {
    /// A `+1` attribute value, or null where the item carries none.
    fn attribute(&self, name: &str) -> Option<Attribute> {
        let key = NSString::from_str(name);
        let value = unsafe { MDItemCopyAttribute(self.0, (&*key as *const NSString).cast()) };
        // `then_some` would build the wrapper first and release a null attribute on the way out.
        (!value.is_null()).then(|| Attribute(value))
    }

    fn string(&self, name: &str) -> Option<String> {
        self.attribute(name)?.string()
    }

    fn number(&self, name: &str) -> Option<f64> {
        self.attribute(name)?.number()
    }

    fn date(&self, name: &str) -> Option<f64> {
        self.attribute(name)?.date()
    }

    fn strings(&self, name: &str) -> Vec<String> {
        self.attribute(name)
            .map(|value| value.strings())
            .unwrap_or_default()
    }
}

struct Attribute(*mut c_void);

impl Drop for Attribute {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

impl Attribute {
    fn is(&self, id: usize) -> bool {
        unsafe { CFGetTypeID(self.0) == id }
    }

    fn string(&self) -> Option<String> {
        if !self.is(unsafe { CFStringGetTypeID() }) {
            return None;
        }
        let text = unsafe { &*self.0.cast::<NSString>() }.to_string();
        (!text.is_empty()).then_some(text)
    }

    fn number(&self) -> Option<f64> {
        self.is(unsafe { CFNumberGetTypeID() })
            .then(|| unsafe { &*self.0.cast::<NSNumber>() }.doubleValue())
    }

    fn date(&self) -> Option<f64> {
        self.is(unsafe { CFDateGetTypeID() })
            .then(|| unsafe { &*self.0.cast::<NSDate>() }.timeIntervalSinceReferenceDate())
    }

    fn strings(&self) -> Vec<String> {
        if !self.is(unsafe { CFArrayGetTypeID() }) {
            return Vec::new();
        }
        let array = unsafe { &*self.0.cast::<NSArray<NSString>>() };
        array.iter().map(|text| text.to_string()).collect()
    }
}

fn pair(item: &Item, width: &str, height: &str) -> Option<(u64, u64)> {
    let (width, height) = (item.number(width)?, item.number(height)?);
    (width > 0. && height > 0.).then_some((width as u64, height as u64))
}

/// Spotlight's attributes for one real file. Empty where the item doesn't exist, its volume
/// isn't indexed, or Spotlight hasn't caught up to it yet.
pub fn meta(path: &Path) -> Meta {
    let Some(text) = path.to_str() else {
        return Meta::default();
    };
    let path = NSString::from_str(text);
    let item = unsafe {
        MDItemCreate(
            std::ptr::null(),
            (&*path as *const NSString).cast::<c_void>(),
        )
    };
    if item.is_null() {
        return Meta::default();
    }
    let item = Item(item);
    Meta {
        content_created: item.date("kMDItemContentCreationDate"),
        last_used: item.date("kMDItemLastUsedDate"),
        pixels: pair(&item, "kMDItemPixelWidth", "kMDItemPixelHeight"),
        color_space: item.string("kMDItemColorSpace"),
        color_profile: item.string("kMDItemProfileName"),
        version: item.string("kMDItemVersion"),
        pages: item
            .number("kMDItemNumberOfPages")
            .map(|pages| pages as u64),
        page: pair(&item, "kMDItemPageWidth", "kMDItemPageHeight"),
        security: item.string("kMDItemSecurityMethod"),
        encoders: item.strings("kMDItemEncodingApplications"),
        category: item.string("kMDItemAppStoreCategory"),
        duration: item.number("kMDItemDurationSeconds"),
    }
}
