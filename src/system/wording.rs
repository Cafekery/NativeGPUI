use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2_foundation::NSString;

static WORDING: OnceLock<fn(&str) -> Option<String>> = OnceLock::new();

/// Rewords every title the crate hands AppKit before it shows. Set once, before the first menu.
pub fn set_wording(reword: fn(&str) -> Option<String>) {
    WORDING.set(reword).ok();
}

pub(crate) fn worded(text: &str) -> Retained<NSString> {
    let reworded = WORDING.get().and_then(|reword| reword(text));
    NSString::from_str(reworded.as_deref().unwrap_or(text))
}
