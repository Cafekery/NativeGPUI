use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

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

    fn attribute(&self, name: &NSString) -> Option<Self> {
        Self::new(unsafe { MDItemCopyAttribute(self.0.as_ptr(), std::ptr::from_ref(name).cast()) })
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

    fn strings(&self) -> Option<&NSArray<NSString>> {
        self.is(unsafe { CFArrayGetTypeID() }).then(|| self.cast())
    }
}

fn pair(item: &Cf, width: &NSString, height: &NSString) -> Option<(u64, u64)> {
    let width = item.attribute(width)?.number()?;
    let height = item.attribute(height)?.number()?;
    (width > 0. && height > 0.).then_some((width as u64, height as u64))
}

/// Spotlight makes this up from the modification date where it has not indexed the item.
pub fn last_used(path: &Path) -> Option<f64> {
    Cf::item(path)?
        .attribute(ns_string!("kMDItemLastUsedDate"))?
        .date()
}

pub(crate) fn kind(path: &Path) -> Option<String> {
    Cf::item(path)?
        .attribute(ns_string!("kMDItemKind"))?
        .string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sharer {
    Me,
    Named(Arc<str>),
}

pub fn sharer(path: &Path) -> Option<Sharer> {
    let item = Cf::item(path)?;
    let string = |name: &NSString| item.attribute(name)?.string();
    sharer_of(
        string(ns_string!("kMDItemSharedItemCurrentUserRole")).as_deref(),
        string(ns_string!("kMDItemOwnerName")),
    )
}

fn sharer_of(role: Option<&str>, owner: Option<String>) -> Option<Sharer> {
    match role {
        Some("NSURLUbiquitousSharedItemRoleOwner") => Some(Sharer::Me),
        _ => owner.map(|name| Sharer::Named(name.into())),
    }
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
    let string = |name: &NSString| item.attribute(name)?.string();
    let number = |name: &NSString| item.attribute(name)?.number();
    let date = |name: &NSString| item.attribute(name)?.date();
    Meta {
        content_created: date(ns_string!("kMDItemContentCreationDate")),
        last_used: date(ns_string!("kMDItemLastUsedDate")),
        pixels: pair(
            &item,
            ns_string!("kMDItemPixelWidth"),
            ns_string!("kMDItemPixelHeight"),
        ),
        color_space: string(ns_string!("kMDItemColorSpace")),
        color_profile: string(ns_string!("kMDItemProfileName")),
        version: string(ns_string!("kMDItemVersion")),
        pages: number(ns_string!("kMDItemNumberOfPages")).map(|pages| pages as u64),
        page: pair(
            &item,
            ns_string!("kMDItemPageWidth"),
            ns_string!("kMDItemPageHeight"),
        ),
        security: string(ns_string!("kMDItemSecurityMethod")),
        encoders: item
            .attribute(ns_string!("kMDItemEncodingApplications"))
            .and_then(|value| {
                Some(
                    value
                        .strings()?
                        .iter()
                        .map(|text| text.to_string())
                        .collect(),
                )
            })
            .unwrap_or_default(),
        category: string(ns_string!("kMDItemAppStoreCategory")),
        duration: number(ns_string!("kMDItemDurationSeconds")),
        values: values(&item),
    }
}

fn values(item: &Cf) -> Vec<(String, String)> {
    let names = Cf::new(unsafe { MDItemCopyAttributeNames(item.0.as_ptr()) });
    let Some(names) = names.as_ref().and_then(Cf::strings) else {
        return Vec::new();
    };
    names
        .iter()
        .filter_map(|name| {
            let value = item.attribute(&name)?;
            let text = value.string().or_else(|| value.text())?;
            Some((name.to_string(), text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_role_is_me_whatever_name_the_item_carries() {
        let owner = Some("NSURLUbiquitousSharedItemRoleOwner");
        let participant = Some("NSURLUbiquitousSharedItemRoleParticipant");
        assert_eq!(sharer_of(owner, Some("Jace".into())), Some(Sharer::Me));
        assert_eq!(sharer_of(owner, None), Some(Sharer::Me));
        assert_eq!(
            sharer_of(participant, Some("William Jones".into())),
            Some(Sharer::Named("William Jones".into()))
        );
        assert_eq!(sharer_of(participant, None), None);
        assert_eq!(sharer_of(None, None), None);
    }
}
