use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2_foundation::NSString;

static WORDING: OnceLock<fn(&str) -> Option<String>> = OnceLock::new();

/// Rewords the titles the crate hands AppKit (menu rows, toolbar labels and tooltips) before
/// they show, so an app can put them in the user's language. Set once, before the first menu.
pub fn set_wording(reword: fn(&str) -> Option<String>) {
    WORDING.set(reword).ok();
}

pub(crate) fn worded(text: &str) -> Retained<NSString> {
    match WORDING.get().and_then(|reword| reword(text)) {
        Some(said) => NSString::from_str(&said),
        None => NSString::from_str(text),
    }
}
