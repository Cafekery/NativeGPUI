use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSBundle, NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization,
    NSString,
};

/// Finder's bundle takes the user's language only in an app whose Info.plist allows mixed
/// localizations; otherwise every framework answers in the app's development language.
static FINDER: LazyLock<Option<Retained<NSBundle>>> = LazyLock::new(|| {
    NSBundle::bundleWithPath(&NSString::from_str(
        "/System/Library/CoreServices/Finder.app",
    ))
});

fn lookup(key: &str) -> Option<String> {
    let key = NSString::from_str(key);
    let found = FINDER.as_ref()?.localizedStringForKey_value_table(
        &key,
        None,
        Some(&NSString::from_str("LocalizableMerged")),
    );
    (*found != *key).then(|| found.to_string())
}

/// Finder's own text for `key`, else `english`.
pub fn finder(key: &str, english: &str) -> String {
    lookup(key).unwrap_or_else(|| english.to_owned())
}

/// The tables whose US English is reworded: Finder's own, then AppKit's menus and toolbar.
const SOURCES: [(&str, &str, &str); 4] = [
    (
        "/System/Library/CoreServices/Finder.app",
        "LocalizableMerged",
        "en.lproj/LocalizableMerged.strings",
    ),
    (
        "/System/Library/Frameworks/AppKit.framework",
        "MenuCommands",
        "MenuCommands.loctable",
    ),
    (
        "/System/Library/Frameworks/AppKit.framework",
        "Toolbar",
        "Toolbar.loctable",
    ),
    (
        "/System/Library/Frameworks/AppKit.framework",
        "Common",
        "Common.loctable",
    ),
];

struct Rewording {
    bundles: Vec<Option<Retained<NSBundle>>>,
    keys: HashMap<String, (usize, String)>,
}

fn plist(path: &str) -> Option<Retained<NSDictionary>> {
    let bytes = std::fs::read(path).ok()?;
    let found = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(&bytes),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    found.downcast::<NSDictionary>().ok()
}

fn english(bundle: &str, file: &str) -> Vec<(String, String)> {
    let resources = NSBundle::bundleWithPath(&NSString::from_str(bundle))
        .and_then(|bundle| bundle.resourcePath())
        .map(|path| path.to_string());
    let Some(table) = resources.and_then(|resources| plist(&format!("{resources}/{file}"))) else {
        return Vec::new();
    };
    let table = match file.ends_with(".loctable") {
        true => table
            .objectForKey(&NSString::from_str("en"))
            .and_then(|en| en.downcast::<NSDictionary>().ok()),
        false => Some(table),
    };
    let Some(table) = table else {
        return Vec::new();
    };
    let text = |object: Retained<AnyObject>| Some(object.downcast::<NSString>().ok()?.to_string());
    table
        .allKeys()
        .iter()
        .filter_map(|key| {
            let value = text(table.objectForKey(&key)?)?;
            Some((value, text(key)?))
        })
        .collect()
}

/// Built only when the user reads something other than US English.
static REWORDING: LazyLock<Option<Rewording>> = LazyLock::new(|| {
    let finder = FINDER.as_ref()?;
    let first = finder.preferredLocalizations().firstObject()?.to_string();
    if first == "en" {
        return None;
    }
    let mut keys = HashMap::new();
    for (at, (bundle, _, file)) in SOURCES.iter().enumerate() {
        for (value, key) in english(bundle, file) {
            keys.entry(value).or_insert((at, key));
        }
    }
    let bundles = SOURCES
        .iter()
        .map(|(bundle, _, _)| NSBundle::bundleWithPath(&NSString::from_str(bundle)))
        .collect();
    Some(Rewording { bundles, keys })
});

/// `english` as Finder or AppKit word it for the user: "Trash" is "Bin" in British English, and
/// in German "Papierkorb". Text neither of them has stays as it is.
pub fn say(english: &str) -> Cow<'_, str> {
    let found = REWORDING.as_ref().and_then(|rewording| {
        let (at, key) = rewording.keys.get(english)?;
        let bundle = rewording.bundles[*at].as_ref()?;
        let key = NSString::from_str(key);
        let table = NSString::from_str(SOURCES[*at].1);
        let said = bundle.localizedStringForKey_value_table(&key, None, Some(&table));
        (*said != *key).then(|| said.to_string())
    });
    found.map_or(Cow::Borrowed(english), Cow::Owned)
}

/// Finder's text for `count` under its plural family `key`: rows `key_R1`… hold `=N` or an
/// empty catch-all, and the first that takes `count` names `key_V1`… .
pub fn plural(key: &str, count: usize, english: &str) -> String {
    (1..)
        .map_while(|at| Some((at, lookup(&format!("{key}_R{at}"))?)))
        .find(|(_, rule)| {
            rule.strip_prefix('=')
                .map_or(rule.is_empty(), |n| n.parse() == Ok(count))
        })
        .and_then(|(at, _)| lookup(&format!("{key}_V{at}")))
        .unwrap_or_else(|| english.to_owned())
}

/// `text` with Finder's slots, `^0` or `^=0` and on, filled from `args`.
pub fn fill(text: &str, args: &[&str]) -> String {
    let mut filled =
        String::with_capacity(text.len() + args.iter().map(|arg| arg.len()).sum::<usize>());
    let mut rest = text;
    while let Some(at) = rest.find('^') {
        filled.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.strip_prefix('=').unwrap_or(after);
        let slot = digits.chars().next().and_then(|digit| digit.to_digit(10));
        match slot.and_then(|slot| args.get(slot as usize)) {
            Some(arg) => {
                filled.push_str(arg);
                rest = &digits[1..];
            }
            None => {
                filled.push('^');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_slots_take_their_own_argument() {
        assert_eq!(fill("^1 of ^0 selected", &["12", "3"]), "3 of 12 selected");
        assert_eq!(fill("^=1 copy ^=0", &["2", "Report"]), "Report copy 2");
        assert_eq!(
            fill("^0, ^1 available", &["1 item"]),
            "1 item, ^1 available"
        );
        assert_eq!(fill("caret ^ alone", &[]), "caret ^ alone");
    }

    #[test]
    fn the_tables_hold_the_words_the_app_shows() {
        let words: Vec<(String, String)> = SOURCES
            .iter()
            .flat_map(|(bundle, _, file)| english(bundle, file))
            .collect();
        for shown in [
            "Move to Trash",
            "Favorites",
            "Trash",
            "Customize Toolbar…",
            "Colors",
        ] {
            assert!(words.iter().any(|(value, _)| value == shown), "{shown}");
        }
    }

    #[test]
    fn a_count_takes_finders_singular_only_at_one() {
        assert_eq!(fill(&plural("SB_COUNT", 1, "^0 items"), &["1"]), "1 item");
        assert_eq!(fill(&plural("SB_COUNT", 0, "^0 items"), &["0"]), "0 items");
        assert_eq!(
            fill(&plural("SB_COUNT", 12, "^0 items"), &["12"]),
            "12 items"
        );
    }
}
