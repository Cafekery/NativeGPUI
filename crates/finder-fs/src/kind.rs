use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::sync::{Arc, LazyLock, RwLock};

use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2_uniform_type_identifiers::{UTTagClassFilenameExtension, UTType};

use crate::entry::{Entry, EntryKind};

const KINDS_HELD: usize = 4096;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSCopyKindStringForTypeInfo(
        kind: u32,
        creator: u32,
        extension: &NSString,
        out: &mut *mut NSString,
    ) -> i32;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub uti: Arc<str>,
    pub label: Arc<str>,
}

/// Each extension's kind, and whether a folder carrying it is a package.
static KINDS: LazyLock<RwLock<HashMap<String, (Kind, bool)>>> = LazyLock::new(Default::default);

// Deprecated in favour of `UTType`, but still the closest public match to Finder's Kind string.
fn ls_label(ext: &str) -> Option<String> {
    if ext.is_empty() {
        return None;
    }
    let mut label = std::ptr::null_mut();
    let status = unsafe { LSCopyKindStringForTypeInfo(0, 0, &NSString::from_str(ext), &mut label) };
    if status != 0 {
        return None;
    }
    let label = unsafe { Retained::from_raw(label) }?.to_string();
    (!label.is_empty()).then_some(label)
}

fn well_known(identifier: &str) -> Option<Retained<UTType>> {
    UTType::typeWithIdentifier(&NSString::from_str(identifier))
}

fn package_variant(ext: &NSString) -> Option<Retained<UTType>> {
    static TYPES: LazyLock<[Option<Retained<UTType>>; 3]> = LazyLock::new(|| {
        ["public.directory", "com.apple.package", "com.apple.bundle"].map(well_known)
    });
    let [directory, package, bundle] = &*TYPES;
    let ty = UTType::typeWithFilenameExtension_conformingToType(ext, directory.as_ref()?)?;
    let packaged = !ty.isDynamic()
        && [package, bundle]
            .into_iter()
            .flatten()
            .any(|base| ty.conformsToType(base));
    packaged.then_some(ty)
}

fn lookup(ext: &str) -> (Kind, bool) {
    let ns_ext = NSString::from_str(ext);
    let package = package_variant(&ns_ext);
    let is_package = package.is_some();
    let ty = package.or_else(|| UTType::typeWithFilenameExtension(&ns_ext));
    let label = ty.as_ref().and_then(|ty| {
        ls_label(ext).or_else(|| {
            let described = ty.localizedDescription()?.to_string();
            let mut chars = described.chars();
            Some(chars.next()?.to_uppercase().chain(chars).collect())
        })
    });
    let kind = Kind {
        uti: ty.map_or_else(
            || "public.data".into(),
            |ty| ty.identifier().to_string().into(),
        ),
        label: label
            .unwrap_or_else(|| {
                if ext.is_empty() {
                    "Document".into()
                } else {
                    format!("{} File", ext.to_ascii_uppercase())
                }
            })
            .into(),
    };
    (kind, is_package)
}

fn cached(ext: &str, resolve: impl FnOnce(&str) -> (Kind, bool)) -> (Kind, bool) {
    let key = if ext.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(ext.to_ascii_lowercase())
    } else {
        Cow::Borrowed(ext)
    };
    if let Some(hit) = KINDS.read().unwrap().get(&*key) {
        return hit.clone();
    }
    let resolved = resolve(&key);
    let mut kinds = KINDS.write().unwrap();
    if kinds.len() >= KINDS_HELD {
        kinds.clear();
    }
    kinds.insert(key.into_owned(), resolved.clone());
    resolved
}

pub fn load_kinds(from: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(from) else {
        return 0;
    };
    let mut kinds = KINDS.write().unwrap();
    let mut read = 0;
    for line in text.lines() {
        let mut parts = line.split('\t');
        let (Some(ext), Some(uti), Some(label), Some(package)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        kinds.entry(ext.to_owned()).or_insert_with(|| {
            let kind = Kind {
                uti: uti.into(),
                label: label.into(),
            };
            (kind, package == "1")
        });
        read += 1;
    }
    read
}

pub fn save_kinds(to: &Path) -> usize {
    let kinds = KINDS.read().unwrap();
    let held = kinds.len();
    let mut text = String::with_capacity(held * 48);
    for (ext, (kind, package)) in kinds.iter() {
        if !ext.contains('\t') && !kind.label.contains('\t') {
            let package = u8::from(*package);
            writeln!(text, "{ext}\t{}\t{}\t{package}", kind.uti, kind.label).unwrap();
        }
    }
    drop(kinds);
    std::fs::write(to, text).map_or(0, |()| held)
}

pub fn kind_of(entry: &Entry) -> Kind {
    static FIXED: LazyLock<[Kind; 3]> = LazyLock::new(|| {
        [
            ("public.folder", "Folder"),
            ("public.symlink", "Alias"),
            ("com.apple.alias-file", "Alias"),
        ]
        .map(|(uti, label)| Kind {
            uti: uti.into(),
            label: label.into(),
        })
    });
    let [folder, symlink, alias] = &*FIXED;
    match entry.kind {
        EntryKind::Directory if !entry.package => folder.clone(),
        EntryKind::Symlink => symlink.clone(),
        EntryKind::File if entry.alias => alias.clone(),
        _ => kind_for_extension(entry.extension().unwrap_or("")),
    }
}

fn kind_for_extension(ext: &str) -> Kind {
    cached(ext, lookup).0
}

pub fn kind_of_path(path: &Path) -> Kind {
    let ext = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .map_or("", |(_, ext)| ext);
    let resolve = |key: &str| {
        let (mut kind, is_package) = lookup(key);
        // Finder names a kind as LaunchServices does, cased as Spotlight is: "ZIP archive".
        if let Some(label) =
            crate::meta::kind(path).filter(|label| label.eq_ignore_ascii_case(&kind.label))
        {
            kind.label = label.into();
        }
        (kind, is_package)
    };
    cached(ext, resolve).0
}

pub fn is_package_name(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| cached(ext, lookup).1)
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

pub fn conforms(uti: &str, category: Category) -> bool {
    match category {
        Category::Any => true,
        _ => category
            .identifier()
            .is_some_and(|wanted| conforms_to(uti, wanted)),
    }
}

pub fn conforms_to(uti: &str, wanted: &str) -> bool {
    type Held = HashMap<String, HashMap<String, bool>>;
    static HELD: LazyLock<RwLock<Held>> = LazyLock::new(Default::default);
    if uti == wanted {
        return true;
    }
    if let Some(&held) = HELD
        .read()
        .unwrap()
        .get(wanted)
        .and_then(|known| known.get(uti))
    {
        return held;
    }
    let conforms = well_known(uti)
        .zip(well_known(wanted))
        .is_some_and(|(ty, base)| ty.conformsToType(&base));
    HELD.write()
        .unwrap()
        .entry(wanted.to_owned())
        .or_default()
        .insert(uti.to_owned(), conforms);
    conforms
}

pub fn extensions(uti: &str) -> Vec<String> {
    well_known(uti)
        .and_then(|ty| {
            ty.tags()
                .objectForKey(unsafe { UTTagClassFilenameExtension })
        })
        .map_or_else(Vec::new, |exts| {
            exts.iter().map(|ext| ext.to_string()).collect()
        })
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
            ext_hidden: false,
            alias: false,
            custom_icon: false,
            dataless: false,
            readable: true,
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
    fn unknown_and_missing_extensions_are_documents() {
        assert_eq!(&*kind_for_extension("zzzqqqxyz123").label, "Document");
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
        for (ext, want) in [("txt", "Plain Text Document"), ("png", "PNG image")] {
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
        assert!(elapsed.as_millis() < 20, "took {elapsed:?}");
    }
}
