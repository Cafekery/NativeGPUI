use std::collections::{HashMap, HashSet};
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

unsafe fn take_string(ptr: *mut c_void) -> Option<String> {
    let s = Retained::from_raw(ptr.cast::<NSString>())?;
    let s = s.to_string();
    (!s.is_empty()).then_some(s)
}

// Deprecated in favour of `UTType`, but still the closest public match to Finder's Kind string.
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

fn package_names() -> &'static RwLock<HashSet<String>> {
    static NAMES: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

fn well_known(identifier: &str) -> Option<Retained<UTType>> {
    UTType::typeWithIdentifier(&NSString::from_str(identifier))
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

fn package_variant(ns_ext: &NSString) -> Option<Retained<UTType>> {
    static TYPES: OnceLock<[Option<Retained<UTType>>; 3]> = OnceLock::new();
    let [directory, package, bundle] = TYPES.get_or_init(|| {
        ["public.directory", "com.apple.package", "com.apple.bundle"].map(well_known)
    });
    let ty = UTType::typeWithFilenameExtension_conformingToType(ns_ext, directory.as_ref()?)?;
    if ty.identifier().to_string().starts_with("dyn.") {
        return None;
    }
    [package, bundle]
        .into_iter()
        .flatten()
        .any(|base| ty.conformsToType(base))
        .then_some(ty)
}

fn lookup(ext: &str) -> (Kind, bool) {
    let ns_ext = NSString::from_str(ext);
    let package = package_variant(&ns_ext);
    let is_package = package.is_some();
    let Some(ty) = package.or_else(|| UTType::typeWithFilenameExtension(&ns_ext)) else {
        let label = match ext.is_empty() {
            true => "Document".into(),
            false => format!("{} File", ext.to_ascii_uppercase()).into(),
        };
        return (
            Kind {
                uti: "public.data".into(),
                label,
            },
            false,
        );
    };
    let kind = Kind {
        uti: ty.identifier().to_string().into(),
        label: label_for(&ty, ext),
    };
    (kind, is_package)
}

fn cached(ext: &str, resolve: impl FnOnce(&str) -> (Kind, bool)) -> Kind {
    let key = ext.to_ascii_lowercase();
    if let Some(hit) = cache().read().unwrap().get(&key) {
        return hit.clone();
    }
    let (kind, is_package) = resolve(&key);
    cache().write().unwrap().insert(key.clone(), kind.clone());
    if is_package {
        package_names().write().unwrap().insert(key);
    }
    kind
}

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
    cached(ext, lookup)
}

pub fn kind_of_path(path: &Path) -> Kind {
    let ext = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .map_or("", |(_, ext)| ext);
    cached(ext, |key| {
        let (mut kind, is_package) = lookup(key);
        // Finder names a kind as LaunchServices does, cased as Spotlight is: "ZIP archive".
        if let Some(label) =
            spotlight_label(path).filter(|label| label.eq_ignore_ascii_case(&kind.label))
        {
            kind.label = label.into();
        }
        (kind, is_package)
    })
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
        assert_eq!(&*kind_for_extension("").label, "Document");
    }

    #[test]
    fn framework_is_package() {
        assert!(is_package_name("CoreFoundation.framework"));
        assert!(is_package_name("Foo.xcodeproj"));
    }

    #[test]
    fn kind_names_are_finders() {
        let dir = std::env::temp_dir().join(format!("finder-fs-kind-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (ext, want) in [
            ("md", "Markdown"),
            ("json", "JSON"),
            ("html", "HTML text"),
            ("csv", "Comma Separated Spreadsheet (.csv)"),
            ("png", "PNG image"),
        ] {
            let path = dir.join(format!("a.{ext}"));
            std::fs::write(&path, b"").unwrap();
            assert_eq!(&*kind_of_path(&path).label, want, "{ext}");
        }
        let app = dir.join("A.app");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(&*kind_of_path(&app).label, "Application");
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
