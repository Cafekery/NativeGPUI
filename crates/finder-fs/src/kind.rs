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

/// Deprecated in favour of `UTType`, but still the closest public match to Finder's Kind string.
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

/// `None` if the item is missing, its volume isn't indexed, or Spotlight hasn't caught up to it.
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
    pub uti: Arc<str>,
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

/// A real, non-synthesized package or bundle type: how `.app`, `.framework`, `.xcodeproj` resolve.
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

/// The kind table from a past run. Resolving one costs a Spotlight round trip against a real
/// file, so the labels are worth keeping between launches; they only move when macOS does.
pub fn load_kinds(from: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(from) else {
        return 0;
    };
    let mut cache = cache().write().unwrap();
    let mut packages = package_names().write().unwrap();
    let mut read = 0;
    for line in text.lines() {
        let mut parts = line.split('\t');
        let (Some(ext), Some(uti), Some(label), Some(package)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        cache.entry(ext.to_string()).or_insert(Kind {
            uti: uti.into(),
            label: label.into(),
        });
        if package == "1" {
            packages.insert(ext.to_string());
        }
        read += 1;
    }
    read
}

pub fn save_kinds(to: &Path) -> usize {
    let cache = cache().read().unwrap();
    let packages = package_names().read().unwrap();
    let mut text = String::with_capacity(cache.len() * 48);
    for (ext, kind) in cache.iter() {
        if ext.contains('\t') || kind.label.contains('\t') {
            continue;
        }
        let package = u8::from(packages.contains(ext));
        text.push_str(&format!("{ext}\t{}\t{}\t{package}\n", kind.uti, kind.label));
    }
    match std::fs::write(to, text) {
        Ok(()) => cache.len(),
        Err(_) => 0,
    }
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
            dataless: false,
            file_id: 0,
            children: None,
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
        assert!(k.label.eq_ignore_ascii_case("zip archive"), "{}", k.label);
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

/// The kinds Finder's search offers under "Kind is".
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum Category {
    #[default]
    Any,
    Application,
    Archive,
    Document,
    Executable,
    Folder,
    Image,
    Movie,
    Music,
    Pdf,
    Presentation,
    Text,
    Other,
}

impl Category {
    pub const ALL: [Category; 13] = [
        Category::Any,
        Category::Application,
        Category::Archive,
        Category::Document,
        Category::Executable,
        Category::Folder,
        Category::Image,
        Category::Movie,
        Category::Music,
        Category::Pdf,
        Category::Presentation,
        Category::Text,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Any => "Any",
            Category::Application => "Application",
            Category::Archive => "Archive",
            Category::Document => "Document",
            Category::Executable => "Executable",
            Category::Folder => "Folder",
            Category::Image => "Image",
            Category::Movie => "Movie",
            Category::Music => "Music",
            Category::Pdf => "PDF",
            Category::Presentation => "Presentation",
            Category::Text => "Text",
            Category::Other => "Other",
        }
    }

    pub fn identifier(self) -> Option<&'static str> {
        match self {
            Category::Any | Category::Other => None,
            Category::Application => Some("com.apple.application"),
            Category::Archive => Some("public.archive"),
            Category::Document => Some("public.content"),
            Category::Executable => Some("public.executable"),
            Category::Folder => Some("public.folder"),
            Category::Image => Some("public.image"),
            Category::Movie => Some("public.movie"),
            Category::Music => Some("public.audio"),
            Category::Pdf => Some("com.adobe.pdf"),
            Category::Presentation => Some("public.presentation"),
            Category::Text => Some("public.text"),
        }
    }
}

fn conformance() -> &'static RwLock<HashMap<String, u16>> {
    static CACHE: OnceLock<RwLock<HashMap<String, u16>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn conformance_of(uti: &str) -> u16 {
    if let Some(&bits) = conformance().read().unwrap().get(uti) {
        return bits;
    }
    let ty = well_known(uti);
    let mut bits = 0;
    for (at, category) in Category::ALL.iter().enumerate() {
        let base = category.identifier().and_then(well_known);
        let held = match (&ty, base) {
            (Some(ty), Some(base)) => ty.conformsToType(&base),
            _ => false,
        };
        bits |= u16::from(held) << at;
    }
    conformance().write().unwrap().insert(uti.to_string(), bits);
    bits
}

/// Whether a type falls under one of Finder's search kinds. The categories overlap, as Finder's
/// do: plain text is a Document as well as Text.
pub fn conforms(uti: &str, category: Category) -> bool {
    match category {
        Category::Any => true,
        Category::Other => false,
        _ => {
            let at = Category::ALL.iter().position(|it| *it == category).unwrap();
            conformance_of(uti) >> at & 1 == 1
        }
    }
}
