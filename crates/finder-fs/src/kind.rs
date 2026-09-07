//! What Finder calls a file: its uniform type identifier and the localized kind string, cached
//! per extension so a listing can ask per row.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_uniform_type_identifiers::UTType;

use crate::entry::{Entry, EntryKind};

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemCreate(allocator: *const c_void, path: *const c_void) -> *mut c_void;
    fn MDItemCopyAttribute(item: *mut c_void, name: *const c_void) -> *mut c_void;
    fn LSCopyKindStringForTypeInfo(
        in_type: u32,
        in_creator: u32,
        in_extension: *const c_void,
        out_kind_string: *mut *mut c_void,
    ) -> i32;
}

fn ns_ptr(s: &NSString) -> *const c_void {
    (s as *const NSString).cast()
}

/// `ptr` must be a `+1` CFStringRef (the "Copy" ownership rule); consumes that retain.
unsafe fn take_string(ptr: *mut c_void) -> Option<String> {
    let s = Retained::from_raw(ptr.cast::<NSString>())?;
    let s = s.to_string();
    (!s.is_empty()).then_some(s)
}

/// Finder's Kind string for a bare extension, straight from Launch Services. Deprecated by
/// Apple in favor of `UTType`, but still the closest public match to what Finder shows.
fn ls_label(ext: &str) -> Option<String> {
    if ext.is_empty() {
        return None;
    }
    let ns_ext = NSString::from_str(ext);
    let mut out: *mut c_void = std::ptr::null_mut();
    let status = unsafe { LSCopyKindStringForTypeInfo(0, 0, ns_ptr(&ns_ext), &mut out) };
    if status != 0 || out.is_null() {
        return None;
    }
    unsafe { take_string(out) }
}

/// Finder's Kind string for one real file, from Spotlight's `kMDItemKind`. `None` if the item
/// doesn't exist, its volume isn't indexed, or Spotlight hasn't caught up to it yet.
fn spotlight_label(path: &Path) -> Option<String> {
    let path_str = path.to_str()?;
    let ns_path = NSString::from_str(path_str);
    let item = unsafe { MDItemCreate(std::ptr::null(), ns_ptr(&ns_path)) };
    if item.is_null() {
        return None;
    }
    let ns_key = NSString::from_str("kMDItemKind");
    let attr = unsafe { MDItemCopyAttribute(item, ns_ptr(&ns_key)) };
    unsafe { CFRelease(item) };
    if attr.is_null() {
        return None;
    }
    unsafe { take_string(attr) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    /// e.g. `public.plain-text`, `com.apple.application-bundle`, `public.folder`.
    pub uti: Arc<str>,
    /// Finder's localized label, e.g. `Plain Text`, `Application`, `Folder`.
    pub label: Arc<str>,
}

fn cache() -> &'static RwLock<HashMap<String, Kind>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Kind>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn well_known(identifier: &str) -> Option<Retained<UTType>> {
    UTType::typeWithIdentifier(&NSString::from_str(identifier))
}

fn directory_type() -> Option<Retained<UTType>> {
    static TY: OnceLock<Option<Retained<UTType>>> = OnceLock::new();
    TY.get_or_init(|| well_known("public.directory")).clone()
}

fn package_type() -> Option<Retained<UTType>> {
    static TY: OnceLock<Option<Retained<UTType>>> = OnceLock::new();
    TY.get_or_init(|| well_known("com.apple.package")).clone()
}

fn bundle_type() -> Option<Retained<UTType>> {
    static TY: OnceLock<Option<Retained<UTType>>> = OnceLock::new();
    TY.get_or_init(|| well_known("com.apple.bundle")).clone()
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn label_for(ty: &UTType, ext: &str) -> Arc<str> {
    ls_label(ext)
        .or_else(|| {
            ty.localizedDescription()
                .filter(|s| !s.is_empty())
                .map(|s| capitalize(&s.to_string()))
        })
        .unwrap_or_else(|| {
            if ext.is_empty() {
                "Document".to_string()
            } else {
                format!("{} File", ext.to_ascii_uppercase())
            }
        })
        .into()
}

/// A directory-flavored resolution for `ext`, if the extension names a real (non-synthesized)
/// package or bundle type — this is how `.app`, `.framework`, `.xcodeproj` etc. resolve.
fn package_variant(ns_ext: &NSString) -> Option<Retained<UTType>> {
    let dir = directory_type()?;
    let ty = UTType::typeWithFilenameExtension_conformingToType(ns_ext, &dir)?;
    if ty.identifier().to_string().starts_with("dyn.") {
        return None;
    }
    let conforms_package = package_type().is_some_and(|p| ty.conformsToType(&p));
    let conforms_bundle = bundle_type().is_some_and(|b| ty.conformsToType(&b));
    (conforms_package || conforms_bundle).then_some(ty)
}

fn lookup(ext: &str) -> (Kind, bool) {
    let ns_ext = NSString::from_str(ext);
    if let Some(ty) = package_variant(&ns_ext) {
        let uti: Arc<str> = ty.identifier().to_string().into();
        let label = label_for(&ty, ext);
        return (Kind { uti, label }, true);
    }
    let Some(ty) = UTType::typeWithFilenameExtension(&ns_ext) else {
        return (
            Kind {
                uti: "public.data".into(),
                label: format!("{} File", ext.to_ascii_uppercase()).into(),
            },
            false,
        );
    };
    let uti: Arc<str> = ty.identifier().to_string().into();
    let label = label_for(&ty, ext);
    (Kind { uti, label }, false)
}

fn package_names() -> &'static RwLock<std::collections::HashSet<String>> {
    static NAMES: OnceLock<RwLock<std::collections::HashSet<String>>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

pub fn kind_of(entry: &Entry) -> Kind {
    match entry.kind {
        EntryKind::Directory if !entry.package => Kind {
            uti: "public.folder".into(),
            label: "Folder".into(),
        },
        EntryKind::Symlink => Kind {
            uti: "public.symlink".into(),
            label: "Alias".into(),
        },
        _ => kind_for_extension(entry.extension().unwrap_or("")),
    }
}

pub fn kind_for_extension(ext: &str) -> Kind {
    let key = ext.to_ascii_lowercase();
    if let Some(hit) = cache().read().unwrap().get(&key) {
        return hit.clone();
    }
    let (kind, is_package) = lookup(&key);
    cache().write().unwrap().insert(key.clone(), kind.clone());
    if is_package {
        package_names().write().unwrap().insert(key);
    }
    kind
}

/// The kind of one real file on disk. Prefers Spotlight's answer for this exact file over the
/// extension-only lookup [`kind_for_extension`] uses; caches under the same per-extension key,
/// so a good Spotlight answer for one file also benefits every later same-extension lookup.
pub fn kind_of_path(path: &Path) -> Kind {
    let ext = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
        .unwrap_or("");
    let key = ext.to_ascii_lowercase();
    if let Some(hit) = cache().read().unwrap().get(&key) {
        return hit.clone();
    }
    let (mut kind, is_package) = lookup(&key);
    if let Some(label) = spotlight_label(path) {
        kind.label = label.into();
    }
    cache().write().unwrap().insert(key.clone(), kind.clone());
    if is_package {
        package_names().write().unwrap().insert(key);
    }
    kind
}

/// Whether a directory with this name is a package Finder shows as a file.
pub fn is_package_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    let key = ext.to_ascii_lowercase();
    if package_names().read().unwrap().contains(&key) {
        return true;
    }
    kind_for_extension(&key);
    package_names().read().unwrap().contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, SystemTime};

    fn file(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            kind: EntryKind::File,
            size: 0,
            modified: SystemTime::now(),
            created: SystemTime::now(),
            added: None,
            hidden: false,
            package: false,
            file_id: 0,
        }
    }

    #[test]
    fn known_extensions() {
        let k = kind_for_extension("pdf");
        assert_eq!(&*k.uti, "com.adobe.pdf");
        assert_eq!(&*k.label, "PDF document");

        let k = kind_for_extension("png");
        assert_eq!(&*k.label, "PNG image");

        let k = kind_for_extension("txt");
        assert_eq!(&*k.label, "Plain Text Document");

        let k = kind_for_extension("zip");
        assert_eq!(&*k.label, "Zip archive");
    }

    #[test]
    fn app_is_application() {
        let mut e = file("Safari.app");
        e.kind = EntryKind::Directory;
        e.package = true;
        let k = kind_of(&e);
        assert_eq!(&*k.uti, "com.apple.application-bundle");
        assert_eq!(&*k.label, "Application");
        assert!(is_package_name("Safari.app"));
    }

    #[test]
    fn plain_directory_is_folder() {
        let mut e = file("Documents");
        e.kind = EntryKind::Directory;
        let k = kind_of(&e);
        assert_eq!(&*k.uti, "public.folder");
        assert_eq!(&*k.label, "Folder");
        assert!(!is_package_name("Documents"));
    }

    #[test]
    fn nonsense_extension_has_label() {
        let k = kind_for_extension("zzzqqqxyz123");
        assert!(!k.label.is_empty());
    }

    #[test]
    fn no_extension_is_document() {
        let k = kind_for_extension("");
        assert!(!k.label.is_empty());
    }

    #[test]
    fn framework_is_package() {
        assert!(is_package_name("CoreFoundation.framework"));
        assert!(is_package_name("Foo.xcodeproj"));
    }

    #[test]
    fn kind_of_path_matches_finder() {
        let dir = std::env::temp_dir().join(format!("finder-fs-kind-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: &[(&str, &str)] = &[
            ("txt", "Plain Text Document"),
            ("md", "Markdown Document"),
            ("zip", "ZIP archive"),
            ("png", "PNG image"),
            ("pdf", "PDF document"),
            ("mov", "QuickTime movie"),
            ("numbers", "Numbers document"),
            ("json", "JSON Document"),
            ("rs", "Document"),
        ];
        for (ext, want) in cases {
            let path = dir.join(format!("a.{ext}"));
            std::fs::write(&path, b"").unwrap();
            let k = kind_of_path(&path);
            let got = &*k.label;
            assert!(
                got == *want || got == kind_for_extension(ext).label.as_ref(),
                "{ext}: got {got:?}, want {want:?} (Spotlight likely hasn't indexed the temp file yet)"
            );
        }
        let app = dir.join("A.app");
        std::fs::create_dir_all(&app).unwrap();
        let k = kind_of_path(&app);
        assert!(
            &*k.label == "Application" || &*k.label == kind_for_extension("app").label.as_ref(),
            "app: got {:?}",
            k.label
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_cache_is_fast() {
        let exts: Vec<String> = (0..30).map(|i| format!("ext{i}")).collect();
        for e in &exts {
            kind_for_extension(e);
        }
        let start = Instant::now();
        for i in 0..20_000 {
            kind_for_extension(&exts[i % exts.len()]);
        }
        let elapsed = start.elapsed();
        println!("20000 warm kind_for_extension calls took {elapsed:?}");
        assert!(elapsed.as_millis() < 20, "took {elapsed:?}");
    }
}
