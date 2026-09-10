use objc2_core_foundation::{
    CFIndex, CFLocale, CFMutableString, CFRange, CFRetained, CFString, CFStringBuiltInEncodings,
    CFStringCompareFlags, CFStringNormalizationForm, kCFAllocatorNull,
};
use std::cmp::Ordering;
use std::sync::OnceLock;

fn current_locale() -> Option<&'static CFLocale> {
    static LOCALE: OnceLock<Option<CFRetained<CFLocale>>> = OnceLock::new();
    LOCALE.get_or_init(CFLocale::current).as_deref()
}

const OPTIONS: CFStringCompareFlags = CFStringCompareFlags::CompareCaseInsensitive
    .union(CFStringCompareFlags::CompareNumerically)
    .union(CFStringCompareFlags::CompareWidthInsensitive)
    .union(CFStringCompareFlags::CompareForcedOrdering);

const FOLD_OPTIONS: CFStringCompareFlags = CFStringCompareFlags::CompareCaseInsensitive
    .union(CFStringCompareFlags::CompareWidthInsensitive)
    .union(CFStringCompareFlags::CompareDiacriticInsensitive);

const PREFIX_OPTIONS: CFStringCompareFlags = CFStringCompareFlags::CompareCaseInsensitive
    .union(CFStringCompareFlags::CompareWidthInsensitive)
    .union(CFStringCompareFlags::CompareNonliteral)
    .union(CFStringCompareFlags::CompareDiacriticInsensitive)
    .union(CFStringCompareFlags::CompareAnchored);

// SAFETY: the returned CFString borrows `s`'s bytes and never escapes the caller's stack frame.
unsafe fn borrow_cf_str(s: &str) -> CFRetained<CFString> {
    CFString::with_bytes_no_copy(
        None,
        s.as_ptr(),
        s.len() as CFIndex,
        CFStringBuiltInEncodings::EncodingUTF8.0,
        false,
        kCFAllocatorNull,
    )
    .expect("str is valid UTF-8")
}

pub fn name_cmp(a: &str, b: &str) -> Ordering {
    let a = unsafe { borrow_cf_str(a) };
    let b = unsafe { borrow_cf_str(b) };
    let range = CFRange::new(0, a.length());
    unsafe { a.compare_with_options_and_locale(Some(&b), range, OPTIONS, current_locale()) }.into()
}

pub fn starts_with(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let name_cf = unsafe { borrow_cf_str(name) };
    let prefix_cf = unsafe { borrow_cf_str(prefix) };
    let range = CFRange::new(0, name_cf.length());
    let mut found = CFRange::new(0, 0);
    unsafe { name_cf.find_with_options(Some(&prefix_cf), range, PREFIX_OPTIONS, &mut found) }
}

pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let borrowed = unsafe { borrow_cf_str(s) };
    let Some(folded) = CFMutableString::new_copy(None, 0, Some(&borrowed)) else {
        return s.to_lowercase();
    };
    unsafe { CFMutableString::normalize(Some(&folded), CFStringNormalizationForm::D) };
    CFMutableString::fold(Some(&folded), FOLD_OPTIONS, current_locale());
    folded.to_string()
}

pub fn sort_names(names: &mut [String]) {
    let cf_strings: Vec<CFRetained<CFString>> =
        names.iter().map(|s| unsafe { borrow_cf_str(s) }).collect();
    let ranges: Vec<CFRange> = cf_strings
        .iter()
        .map(|s| CFRange::new(0, s.length()))
        .collect();
    let locale = current_locale();
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_unstable_by(|&i, &j| {
        unsafe {
            cf_strings[i].compare_with_options_and_locale(
                Some(&cf_strings[j]),
                ranges[i],
                OPTIONS,
                locale,
            )
        }
        .into()
    });
    drop(cf_strings);

    let mut originals: Vec<String> = names.iter_mut().map(std::mem::take).collect();
    for (dst, &src) in names.iter_mut().zip(&order) {
        *dst = std::mem::take(&mut originals[src]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn prefix_is_case_and_width_insensitive() {
        assert!(starts_with("README.md", "read"));
        assert!(starts_with("Ärger", "ärger"));
        assert!(starts_with("Brewfile", "brewfile"));
        assert!(!starts_with("backup.sh", "README"));
        assert!(!starts_with("file 2", "file 10"));
    }

    #[test]
    fn prefix_ignores_how_the_name_is_composed() {
        assert!(starts_with("cafe\u{301}.txt", "caf\u{e9}"));
        assert!(starts_with("caf\u{e9}.txt", "cafe\u{301}"));
        assert!(starts_with("cafe\u{301}.txt", "cafe"));
        assert!(starts_with("cafe.txt", "caf\u{e9}"));
        assert!(!starts_with("cafe.txt", "cafx"));
    }

    #[test]
    fn folding_meets_in_one_form() {
        assert_eq!(fold("cafe\u{301}"), fold("caf\u{e9}"));
        assert_eq!(fold("CAF\u{c9}"), fold("cafe"));
        assert_eq!(fold("README.md"), "readme.md");
    }

    #[test]
    fn numeric_order() {
        assert_eq!(name_cmp("file 2", "file 10"), Ordering::Less);
        assert_eq!(name_cmp("file 10", "file 100"), Ordering::Less);
    }

    #[test]
    fn case_insensitive_but_stable_ordering() {
        assert_eq!(name_cmp("File A", "file a"), name_cmp("File A", "file a"));
        assert_eq!(name_cmp("a", "B"), Ordering::Less);
        assert_eq!(name_cmp("B", "c"), Ordering::Less);
    }

    #[test]
    fn diacritics_sort_next_to_base_letter() {
        assert_eq!(name_cmp("e", "e"), Ordering::Equal);
        assert_ne!(name_cmp("e", "é"), Ordering::Equal);
        assert_eq!(name_cmp("d", "é"), Ordering::Less);
        assert_eq!(name_cmp("é", "f"), Ordering::Less);
    }

    #[test]
    fn dot_and_underscore_prefixes() {
        let mut names = vec![
            "_underscore".to_string(),
            ".hidden".to_string(),
            "file".to_string(),
        ];
        sort_names(&mut names);
        assert_eq!(names, vec!["_underscore", ".hidden", "file"]);
    }

    #[test]
    fn bench_20k_names() {
        let mut names: Vec<String> = (0..20_000)
            .map(|i| match i % 3 {
                0 => format!("File {i}"),
                1 => format!("document_{i}.txt"),
                _ => format!("IMG_{i:05}.JPG"),
            })
            .collect();
        let start = Instant::now();
        sort_names(&mut names);
        let elapsed = start.elapsed();
        println!("sorted 20000 names in {:?}", elapsed);
        assert!(elapsed.as_millis() < 200, "took {:?}", elapsed);
    }
}
