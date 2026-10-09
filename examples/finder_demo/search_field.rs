//! The toolbar's search field; with the private API, Finder's own token field from Suggestions.framework.

use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_app_kit::NSSearchField;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDateFormatter, NSLocale, NSObjectProtocol, NSString, ns_string,
};

use crate::chrome::toolbar::SEARCH_ID;
use crate::terms::{Search, Term};

const KIND: isize = 3;
const FILENAME: isize = 4;
const TAG: isize = 9;
/// Everything is one below `NSIntegerMax`, dates two.
const DATE: isize = isize::MAX - 2;

pub(crate) fn build(mtm: MainThreadMarker) -> Retained<NSSearchField> {
    #[cfg(feature = "private-api")]
    if let Some(field) = finder::field() {
        return field;
    }
    NSSearchField::new(mtm)
}

#[cfg(feature = "private-api")]
mod finder {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::sync::LazyLock;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2::{msg_send, sel};
    use objc2_app_kit::NSSearchField;

    static LOADED: LazyLock<bool> = LazyLock::new(|| {
        unsafe extern "C" {
            fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        }
        const RTLD_LAZY: c_int = 1;
        let path = c"/System/Library/PrivateFrameworks/Suggestions.framework/Suggestions";
        !unsafe { dlopen(path.as_ptr(), RTLD_LAZY) }.is_null()
    });

    fn shared(class: &CStr, selector: Sel) -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(class)?;
        if !class.metaclass().responds_to(selector) {
            return None;
        }
        unsafe { msg_send![class, performSelector: selector] }
    }

    pub(super) fn field() -> Option<Retained<NSSearchField>> {
        if !*LOADED {
            return None;
        }
        let class = AnyClass::get(c"SGTSearchField")?;
        let genius = shared(c"SGTQueryGenius", sel!(geniusForFiles))?;
        let suggesters = shared(c"SGTSuggester", sel!(suggestersForFiles))?;
        let field: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        let field = field?;
        if !super::responds(
            &field,
            &[sel!(setGenius:), sel!(setSuggesters:), sel!(setUseTokens:)],
        ) {
            return None;
        }
        unsafe {
            let () = msg_send![&field, setGenius: &*genius];
            let () = msg_send![&field, setSuggesters: &*suggesters];
            let () = msg_send![&field, setUseTokens: true];
        }
        field.downcast::<NSSearchField>().ok()
    }
}

fn field(window: &Window) -> Option<Retained<NSSearchField>> {
    native_gpui::toolbar_item_view(window, SEARCH_ID)?
        .downcast::<NSSearchField>()
        .ok()
}

fn responds(object: &AnyObject, selectors: &[Sel]) -> bool {
    selectors
        .iter()
        .all(|&selector| object.class().responds_to(selector))
}

fn string(object: &AnyObject, selector: Sel) -> Option<String> {
    if !responds(object, &[selector]) {
        return None;
    }
    let value: Option<Retained<AnyObject>> =
        unsafe { msg_send![object, performSelector: selector] };
    Some(value?.downcast::<NSString>().ok()?.to_string())
}

/// The field's search, and the query Spotlight would run for it.
pub(crate) fn typed(window: &Window, text: &str) -> (Search, Option<String>) {
    field(window)
        .filter(|field| field.respondsToSelector(sel!(suggestions)))
        .map_or_else(
            || (Search::typed(text.to_owned()), None),
            |field| read(&field),
        )
}

fn read(field: &NSSearchField) -> (Search, Option<String>) {
    let tokens: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![field, suggestions] };
    let terms = tokens
        .iter()
        .flat_map(|tokens| tokens.iter())
        .filter_map(|token| term(&token))
        .collect();
    let text = field
        .cell()
        .and_then(|cell| string(&cell, sel!(untokenizedStringValue)))
        .unwrap_or_else(|| field.stringValue().to_string());
    (Search { terms, text }, string(field, sel!(queryString)))
}

fn term(token: &AnyObject) -> Option<Term> {
    if !responds(token, &[sel!(scope), sel!(attributeValues)]) {
        return None;
    }
    let scope: isize = unsafe { msg_send![token, scope] };
    let values: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![token, attributeValues] };
    let value = values
        .and_then(|values| values.firstObject())
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string());
    let display = string(token, sel!(displayName));
    Some(match scope {
        FILENAME => Term::Name(value.or(display)?),
        KIND => Term::Kind(value?),
        TAG => Term::Tag(value.or(display)?),
        DATE => {
            let (from, to) = dates(&string(token, sel!(queryStringFragment))?)?;
            Term::Date(from, to)
        }
        _ => Term::Everything(display.or(value)?),
    })
}

/// Spotlight's query writes a date token's range as two local `$time.iso(...)` stamps.
fn dates(query: &str) -> Option<(f64, f64)> {
    let formatter = NSDateFormatter::new();
    formatter.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(ns_string!(
        "en_US_POSIX"
    ))));
    formatter.setDateFormat(Some(ns_string!("yyyy-MM-dd'T'HH:mm:ss")));
    let mut stamps = query
        .split("$time.iso(")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .filter_map(|stamp| formatter.dateFromString(&NSString::from_str(stamp.trim())))
        .map(|date| date.timeIntervalSince1970());
    Some((stamps.next()?, stamps.next()?))
}

pub(crate) fn clear(window: &Window) {
    if let Some(field) = field(window).filter(|field| field.stringValue().length() > 0) {
        field.setStringValue(&NSString::new());
    }
}
