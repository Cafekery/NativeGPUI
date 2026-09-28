use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

use objc2_foundation::{
    NSData, NSDictionary, NSFileManager, NSNumber, NSPropertyListMutabilityOptions,
    NSPropertyListSerialization, NSString, NSURL, NSURLHasHiddenExtensionKey,
};

const SYSTEM: u8 = 0;
const HIDE: u8 = 1;
const SHOW: u8 = 2;

static EXTENSIONS: AtomicU8 = AtomicU8::new(SYSTEM);

/// The folder names LaunchServices shows in the user's language wherever the system keeps them.
static TRANSLATED: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let path = "/System/Library/CoreServices/SystemFolderLocalizations/en.lproj/SystemFolderLocalizations.strings";
    let Ok(bytes) = std::fs::read(path) else {
        return HashSet::new();
    };
    let names = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(&bytes),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    };
    names
        .ok()
        .and_then(|names| names.downcast::<NSDictionary>().ok())
        .map_or_else(HashSet::new, |names| {
            names
                .allKeys()
                .iter()
                .filter_map(|name| Some(name.downcast::<NSString>().ok()?.to_string()))
                .collect()
        })
});

/// A name as Finder draws it: `:` as the `/` the user typed, a control character as `?`, and a
/// direction override closed before the extension, so `\u{202e}gnp.txt` can't pass for a
/// `.png`.
pub fn display(name: &str) -> Cow<'_, str> {
    let mapped = |c: char| c == ':' || (c.is_control() && c != '\t');
    let open = name.chars().fold(0usize, |open, c| match c {
        '\u{202a}' | '\u{202b}' | '\u{202d}' | '\u{202e}' => open + 1,
        '\u{202c}' => open.saturating_sub(1),
        _ => open,
    });
    if open == 0 && !name.chars().any(mapped) {
        return Cow::Borrowed(name);
    }
    let (stem, extension) = match name.rfind('.').filter(|&dot| dot > 0 && open > 0) {
        Some(dot) => name.split_at(dot),
        None => (name, ""),
    };
    let map = |c: char| match c {
        ':' => '/',
        c if mapped(c) => '?',
        c => c,
    };
    Cow::Owned(
        stem.chars()
            .map(map)
            .chain(std::iter::repeat_n('\u{202c}', open))
            .chain(extension.chars().map(map))
            .collect(),
    )
}

/// Overrides "Show all filename extensions" for every name shown here; `None` follows the
/// system-wide setting, the only one LaunchServices reads.
pub fn show_all_extensions(show: Option<bool>) {
    let mode = match show {
        None => SYSTEM,
        Some(false) => HIDE,
        Some(true) => SHOW,
    };
    EXTENSIONS.store(mode, Ordering::Relaxed);
}

fn launch_services(path: &Path) -> String {
    let path = NSString::from_str(&path.to_string_lossy());
    display(
        &NSFileManager::defaultManager()
            .displayNameAtPath(&path)
            .to_string(),
    )
    .into_owned()
}

/// LaunchServices' own answer, which counts an application's extension as hidden.
fn hides_extension(path: &Path) -> bool {
    let path = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath_isDirectory(&path, false);
    let mut hidden = None;
    let _ = unsafe { url.getResourceValue_forKey_error(&mut hidden, NSURLHasHiddenExtensionKey) };
    hidden
        .and_then(|hidden| hidden.downcast::<NSNumber>().ok())
        .is_some_and(|hidden| hidden.boolValue())
}

/// The name Finder shows for a path: LaunchServices' display name, which leaves off an
/// extension the item hides or an application's, keeps it under "Show all filename extensions",
/// and drops `.localized`.
pub fn of_path(path: &Path) -> String {
    match (
        EXTENSIONS.load(Ordering::Relaxed),
        path.parent(),
        path.file_name(),
    ) {
        (SYSTEM, ..) | (_, None, _) | (_, _, None) => launch_services(path),
        (mode, Some(dir), Some(name)) => {
            named(mode, dir, &name.to_string_lossy(), hides_extension(path))
        }
    }
}

/// `of_path` for an item a listing read, asked of LaunchServices only when the item can lose
/// part of its name or wear a translated one: the answer costs about 40 µs an item.
pub fn shown(dir: &Path, name: &str, ext_hidden: bool) -> String {
    named(EXTENSIONS.load(Ordering::Relaxed), dir, name, ext_hidden)
}

fn named(mode: u8, dir: &Path, name: &str, ext_hidden: bool) -> String {
    let split = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty());
    let app = split.is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("app"));
    if split.is_some_and(|(_, ext)| ext == "localized") || TRANSLATED.contains(name) {
        return launch_services(&dir.join(name));
    }
    match (mode, split) {
        (SYSTEM, Some(_)) if ext_hidden || app => launch_services(&dir.join(name)),
        (HIDE, Some((stem, _))) if ext_hidden || app && hides_extension(&dir.join(name)) => {
            display(stem).into_owned()
        }
        _ => display(name).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colon_draws_as_the_separator_the_user_typed() {
        assert_eq!(display("colon:in:name.txt"), "colon/in/name.txt");
        assert_eq!(display("plain.txt"), "plain.txt");
    }

    #[test]
    fn a_control_character_keeps_the_name_on_one_line() {
        assert_eq!(display("newline\nin-name.txt"), "newline?in-name.txt");
        assert_eq!(display("bell\u{7}.txt"), "bell?.txt");
        assert_eq!(display("tab\tin-name.txt"), "tab\tin-name.txt");
        assert_eq!(
            display("caf\u{e9} \u{1f600}.txt"),
            "caf\u{e9} \u{1f600}.txt"
        );
    }

    #[test]
    fn a_direction_override_is_closed_before_the_extension() {
        assert_eq!(display("\u{202e}gnp.txt"), "\u{202e}gnp\u{202c}.txt");
        assert_eq!(display("\u{202e}a\u{202c}b.txt"), "\u{202e}a\u{202c}b.txt");
        assert_eq!(
            display("\u{202d}\u{202e}x"),
            "\u{202d}\u{202e}x\u{202c}\u{202c}"
        );
    }

    #[test]
    fn a_name_with_nothing_to_map_is_not_copied() {
        assert!(matches!(display("Report.pdf"), Cow::Borrowed(_)));
        assert!(matches!(display("a:b"), Cow::Owned(_)));
    }

    #[test]
    fn only_names_launch_services_leaves_alone_skip_it() {
        let dir = std::env::temp_dir().join(format!("finder-fs-shown-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Loc.localized")).unwrap();
        std::fs::create_dir_all(dir.join("Dir.app")).unwrap();
        for name in [
            "Doc.txt",
            "Two.pdf.txt",
            "Up.APP",
            "Trail.",
            "noext",
            "a:b.txt",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::os::unix::fs::symlink("/System/Applications/Calculator.app", dir.join("Calc.app"))
            .unwrap();
        for name in std::fs::read_dir(&dir).unwrap().flatten() {
            let name = name.file_name().to_string_lossy().into_owned();
            assert_eq!(
                shown(&dir, &name, false),
                of_path(&dir.join(&name)),
                "{name}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_override_hides_or_shows_extensions_whatever_the_system_does() {
        let dir = std::env::temp_dir().join(format!("finder-fs-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Doc.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("/System/Applications/Calculator.app", dir.join("Calc.app"))
            .unwrap();
        assert_eq!(named(HIDE, &dir, "Doc.txt", true), "Doc");
        assert_eq!(named(HIDE, &dir, "Doc.txt", false), "Doc.txt");
        assert_eq!(named(SHOW, &dir, "Doc.txt", true), "Doc.txt");
        assert_eq!(named(SHOW, &dir, "Calc.app", false), "Calc.app");
        assert_eq!(named(HIDE, &dir, "Calc.app", false), "Calc");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
