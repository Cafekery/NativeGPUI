use std::sync::LazyLock;

use objc2::rc::Retained;
use objc2_foundation::{NSBundle, NSString};

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
    fn a_count_takes_finders_singular_only_at_one() {
        assert_eq!(fill(&plural("SB_COUNT", 1, "^0 items"), &["1"]), "1 item");
        assert_eq!(fill(&plural("SB_COUNT", 0, "^0 items"), &["0"]), "0 items");
        assert_eq!(
            fill(&plural("SB_COUNT", 12, "^0 items"), &["12"]),
            "12 items"
        );
    }
}
