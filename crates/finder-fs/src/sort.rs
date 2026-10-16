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

const ENTRY_OPTIONS: CFStringCompareFlags =
    CFStringCompareFlags::CompareCaseInsensitive.union(CFStringCompareFlags::CompareNonliteral);

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

fn cf_cmp(a: &CFString, b: &CFString, locale: Option<&CFLocale>) -> Ordering {
    let range = CFRange::new(0, a.length());
    unsafe { a.compare_with_options_and_locale(Some(b), range, OPTIONS, locale) }.into()
}

pub fn name_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = unsafe { (borrow_cf_str(a), borrow_cf_str(b)) };
    cf_cmp(&a, &b, current_locale())
}

pub fn starts_with(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let (name, prefix) = unsafe { (borrow_cf_str(name), borrow_cf_str(prefix)) };
    let range = CFRange::new(0, name.length());
    let mut found = CFRange::new(0, 0);
    unsafe { name.find_with_options(Some(&prefix), range, PREFIX_OPTIONS, &mut found) }
}

/// Two spellings a case-insensitive volume finds one entry under.
pub(crate) fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = unsafe { (borrow_cf_str(a), borrow_cf_str(b)) };
    let range = CFRange::new(0, a.length());
    let order: Ordering =
        unsafe { a.compare_with_options_and_locale(Some(&b), range, ENTRY_OPTIONS, None) }.into();
    order == Ordering::Equal
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

/// Each name's place in `name_cmp` order, equal names sharing one, so a sort compares integers.
pub fn ranks<'a>(names: impl Iterator<Item = &'a str>) -> Vec<u32> {
    let names: Vec<CFRetained<CFString>> = names.map(|s| unsafe { borrow_cf_str(s) }).collect();
    let locale = current_locale();
    let cmp = |a: usize, b: usize| cf_cmp(&names[a], &names[b], locale);
    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_unstable_by(|&a, &b| cmp(a, b));
    let mut ranks = vec![0; names.len()];
    for pair in order.windows(2) {
        ranks[pair[1]] = ranks[pair[0]] + u32::from(cmp(pair[0], pair[1]) != Ordering::Equal);
    }
    ranks
}

/// Positions of `n` items ordered by `by`, then name (reversed when `descending`), then index.
pub fn order<'a>(
    n: usize,
    by: impl Fn(usize, usize) -> Ordering,
    name: impl Fn(usize) -> &'a str,
    descending: bool,
) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| by(a, b));
    for run in order.chunk_by_mut(|&a, &b| by(a, b) == Ordering::Equal) {
        if run.len() < 2 {
            continue;
        }
        let mut ranked: Vec<(u32, usize)> = ranks(run.iter().map(|&at| name(at)))
            .into_iter()
            .zip(run.iter().copied())
            .collect();
        if descending {
            ranked.sort_by_key(|&(rank, _)| std::cmp::Reverse(rank));
        } else {
            ranked.sort_by_key(|&(rank, _)| rank);
        }
        for (slot, (_, at)) in run.iter_mut().zip(ranked) {
            *slot = at;
        }
    }
    order
}

/// The first `k` positions of `order`.
pub fn first<'a>(
    n: usize,
    k: usize,
    by: impl Fn(usize, usize) -> Ordering,
    name: impl Fn(usize) -> &'a str,
    descending: bool,
) -> Vec<usize> {
    let before = |a: usize, b: usize| {
        by(a, b)
            .then_with(|| {
                if descending {
                    name_cmp(name(b), name(a))
                } else {
                    name_cmp(name(a), name(b))
                }
            })
            .then(a.cmp(&b))
    };
    let mut kept: Vec<usize> = Vec::with_capacity(k + 1);
    for at in 0..n {
        if kept.len() == k && kept.last().is_none_or(|&last| before(at, last).is_gt()) {
            continue;
        }
        let slot = kept.partition_point(|&held| before(held, at).is_lt());
        kept.insert(slot, at);
        kept.truncate(k);
    }
    kept
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
    fn case_insensitive_ordering() {
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
        assert_eq!(
            ranks(["file", ".hidden", "_underscore"].into_iter()),
            [2, 1, 0]
        );
    }

    #[test]
    fn ranks_order_as_name_cmp_does() {
        let names = [
            "_underscore",
            ".hidden",
            "file",
            "File",
            "file 10",
            "file 2",
            "é",
            "e",
            "f",
            "file",
        ];
        let ranked = ranks(names.into_iter());
        for (a, x) in names.iter().enumerate() {
            for (b, y) in names.iter().enumerate() {
                assert_eq!(ranked[a].cmp(&ranked[b]), name_cmp(x, y), "{x} {y}");
            }
        }
    }

    #[test]
    fn a_first_screenful_is_the_head_of_the_whole_order() {
        let stems = [
            "file", "File", "file 2", "file 10", "é", "e", "_x", ".x", "Zed", "zed",
        ];
        let mut seed = 7u64;
        let mut next = |bound: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % bound
        };
        let items: Vec<(u64, String)> = (0..400)
            .map(|_| {
                let stem = stems[next(stems.len() as u64) as usize];
                (next(6), format!("{stem} {}", next(40)))
            })
            .collect();
        for descending in [false, true] {
            let by = |a: usize, b: usize| items[b].0.cmp(&items[a].0);
            let name = |at: usize| items[at].1.as_str();
            let whole = order(items.len(), by, name, descending);
            let mut expected: Vec<usize> = (0..items.len()).collect();
            expected.sort_by(|&a, &b| {
                let names = name_cmp(name(a), name(b));
                by(a, b).then(if descending { names.reverse() } else { names })
            });
            assert_eq!(whole, expected, "descending {descending}");
            for k in [0, 1, 7, 64, 399, 400, 450] {
                let head = &whole[..k.min(whole.len())];
                assert_eq!(first(items.len(), k, by, name, descending), head, "k {k}");
            }
        }
    }

    #[test]
    fn bench_20k_names() {
        let names: Vec<String> = (0..20_000)
            .map(|i| match i % 3 {
                0 => format!("File {i}"),
                1 => format!("document_{i}.txt"),
                _ => format!("IMG_{i:05}.JPG"),
            })
            .collect();
        let start = Instant::now();
        ranks(names.iter().map(String::as_str));
        let elapsed = start.elapsed();
        println!("ranked 20000 names in {:?}", elapsed);
        assert!(elapsed.as_millis() < 200, "took {:?}", elapsed);
    }
}
