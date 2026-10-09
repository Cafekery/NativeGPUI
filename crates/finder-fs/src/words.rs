use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSBundle, NSDictionary, NSString, ns_string};

/// Finder's bundle answers in the user's language only if the app's Info.plist allows mixed localizations.
static FINDER: LazyLock<Option<Retained<NSBundle>>> = LazyLock::new(|| bundle(FINDER_PATH));

const FINDER_PATH: &str = "/System/Library/CoreServices/Finder.app";
const APPKIT_PATH: &str = "/System/Library/Frameworks/AppKit.framework";

fn bundle(path: &str) -> Option<Retained<NSBundle>> {
    NSBundle::bundleWithPath(&NSString::from_str(path))
}

fn lookup(key: &str) -> Option<String> {
    let key = NSString::from_str(key);
    let found = FINDER.as_ref()?.localizedStringForKey_value_table(
        &key,
        None,
        Some(ns_string!("LocalizableMerged")),
    );
    (*found != *key).then(|| found.to_string())
}

pub fn finder(key: &str, english: &str) -> String {
    lookup(key).unwrap_or_else(|| english.to_owned())
}

/// (bundle, table, file) in lookup priority order.
const SOURCES: [(&str, &str, &str); 4] = [
    (
        FINDER_PATH,
        "LocalizableMerged",
        "en.lproj/LocalizableMerged.strings",
    ),
    (APPKIT_PATH, "MenuCommands", "MenuCommands.loctable"),
    (APPKIT_PATH, "Toolbar", "Toolbar.loctable"),
    (APPKIT_PATH, "Common", "Common.loctable"),
];

struct Rewording {
    bundles: Vec<Option<Retained<NSBundle>>>,
    keys: HashMap<String, (usize, String)>,
}

/// (US English text, key) pairs of one table.
fn english(bundle: &NSBundle, file: &str) -> Vec<(String, String)> {
    let table = bundle
        .resourcePath()
        .and_then(|resources| std::fs::read(format!("{resources}/{file}")).ok())
        .and_then(|bytes| crate::plist(&bytes)?.downcast::<NSDictionary>().ok());
    let table = match file.ends_with(".loctable") {
        true => table.and_then(|table| {
            table
                .objectForKey(ns_string!("en"))?
                .downcast::<NSDictionary>()
                .ok()
        }),
        false => table,
    };
    let Some(table) = table else {
        return Vec::new();
    };
    texts(&table).collect()
}

fn texts(table: &NSDictionary) -> impl Iterator<Item = (String, String)> + '_ {
    let text = |object: Retained<AnyObject>| Some(object.downcast::<NSString>().ok()?.to_string());
    table
        .allKeys()
        .into_iter()
        .filter_map(move |key| Some((text(table.objectForKey(&key)?)?, text(key)?)))
}

static REWORDING: LazyLock<Option<Rewording>> = LazyLock::new(|| {
    let first = FINDER.as_ref()?.preferredLocalizations().firstObject()?;
    if *first == *ns_string!("en") {
        return None;
    }
    let bundles: Vec<_> = SOURCES.iter().map(|(path, _, _)| bundle(path)).collect();
    let mut keys = HashMap::new();
    for (at, (bundle, (_, _, file))) in bundles.iter().zip(&SOURCES).enumerate() {
        for (value, key) in bundle.iter().flat_map(|bundle| english(bundle, file)) {
            keys.entry(value).or_insert((at, key));
        }
    }
    Some(Rewording { bundles, keys })
});

/// `english` as Finder or AppKit word it in the user's language; unknown text stays as it is.
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

/// Rows `key_R1`… hold `=N` or an empty catch-all; the first that takes `count` names `key_V1`….
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

/// Fills Finder's `^0` / `^=0` slots from `args`.
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

const DESKTOP_SERVICES_UI: &str =
    "/System/Library/PrivateFrameworks/DesktopServicesUI.framework/Resources/Localizable.loctable";

#[derive(Default)]
struct Customize {
    said: HashMap<String, String>,
    symbols: HashMap<String, String>,
}

static CUSTOMIZE: LazyLock<Customize> = LazyLock::new(|| {
    let table = std::fs::read(DESKTOP_SERVICES_UI)
        .ok()
        .and_then(|bytes| crate::plist_dict(&bytes));
    let Some(table) = table else {
        return Customize::default();
    };
    let words = |language: &NSString| {
        table
            .objectForKey(language)?
            .downcast::<NSDictionary>()
            .ok()
    };
    let finders = FINDER
        .as_ref()
        .and_then(|finder| words(&*finder.preferredLocalizations().firstObject()?));
    let said: HashMap<_, _> = finders
        .iter()
        .flat_map(|finders| texts(finders))
        .map(|(text, key)| (key, text))
        .collect();
    let any = table.allKeys().iter().find_map(|language| words(&language));
    let symbols = any
        .iter()
        .flat_map(|any| texts(any))
        .filter_map(|(_, key)| {
            let (english, symbol) = key.strip_suffix(']')?.rsplit_once(" [AX for symbol ")?;
            let label = said.get(&key).map_or(english, String::as_str);
            Some((symbol.to_owned(), label.to_owned()))
        })
        .collect();
    Customize { said, symbols }
});

pub fn customize(key: &str, english: &str) -> String {
    CUSTOMIZE
        .said
        .get(key)
        .cloned()
        .unwrap_or_else(|| english.to_owned())
}

pub fn symbol_label(symbol: &str) -> Option<String> {
    CUSTOMIZE.symbols.get(symbol).cloned()
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
            .flat_map(|(path, _, file)| english(&bundle(path).unwrap(), file))
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
