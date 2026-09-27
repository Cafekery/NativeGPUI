use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_uniform_type_identifiers::UTType;

/// Finder falls back to these only when IconServices cannot say.
const FAMILIES: [(&str, &str); 11] = [
    ("com.apple.mac.notched-laptop", "macbook.gen2"),
    ("com.apple.mac.laptop", "macbook.gen1"),
    ("com.apple.macmini", "macmini.gen2"),
    ("com.apple.imac", "desktopcomputer"),
    ("com.apple.macpro-2019-rackmount", "macpro.gen3.server"),
    ("com.apple.macpro", "macpro.gen3"),
    ("com.apple.macstudio", "macstudio"),
    ("com.apple.xserve", "doc"),
    ("com.apple.airport-extreme-tower", "airport.extreme.tower"),
    ("com.apple.time-capsule", "airport.extreme"),
    ("public.generic-pc", "pc"),
];

enum Said {
    Model(String),
    /// sharingd lists the server without one.
    Unknown,
    /// Bonjour holds no `_device-info` record for it.
    Unrecorded,
}

/// Calls `found` with the model of each host a network volume was mounted from, as each answer
/// lands within `wait`; `None` is a Bonjour server without one, which Finder draws as a Mac.
///
/// sharingd holds Finder's model: the one an SMB server gave the kernel, or a Bonjour name's
/// `_device-info` record, which Bonjour itself returns in milliseconds where sharingd can take a
/// minute after a mount.
pub fn models(hosts: &[String], wait: Duration, mut found: impl FnMut(&str, Option<&str>)) {
    let deadline = Instant::now() + wait;
    let (tx, answers) = channel();
    let browsing = browse(tx.clone());
    let mut left: Vec<(String, &String, [bool; 2])> = hosts
        .iter()
        .map(|host| (named(host), host, [false; 2]))
        .collect();
    for name in left.iter().map(|(name, ..)| name.clone()) {
        let Some(instance) = instance(&name).map(str::to_owned) else {
            continue;
        };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let model = device_model(&instance, Instant::now() + Duration::from_secs(3));
            tx.send((name, model.map_or(Said::Unrecorded, Said::Model)))
                .ok();
        });
    }
    drop(tx);
    while !left.is_empty() {
        let wait = deadline.saturating_duration_since(Instant::now());
        let Ok((host, said)) = answers.recv_timeout(wait) else {
            break;
        };
        let Some(at) = left
            .iter()
            .position(|(name, ..)| name.eq_ignore_ascii_case(&host))
        else {
            continue;
        };
        let [unknown, unrecorded] = &mut left[at].2;
        let model = match said {
            Said::Model(model) => Some(model),
            Said::Unknown => {
                *unknown = true;
                None
            }
            Said::Unrecorded => {
                *unrecorded = true;
                None
            }
        };
        if model.is_some() || *unknown && *unrecorded {
            autoreleasepool(|_| found(left.swap_remove(at).1, model.as_deref()));
        }
    }
    if let Some((browser, queue, shared)) = browsing {
        unsafe {
            dispatch_sync_f(queue, browser, shared.1.invalidate);
            CFRelease(browser);
            dispatch_release(queue);
        }
    }
}

/// Finder titles a Bonjour server by its instance name.
pub(crate) fn title(host: &str) -> String {
    let name = named(host);
    instance(&name).map_or_else(|| name.clone(), str::to_owned)
}

/// A mount source percent-encodes a Bonjour name; sharingd's URL host does not.
fn named(host: &str) -> String {
    let decoded = NSString::from_str(host).stringByRemovingPercentEncoding();
    decoded.map_or_else(|| host.to_owned(), |host| host.to_string())
}

fn instance(name: &str) -> Option<&str> {
    Some(name.strip_suffix("._tcp.local")?.rsplit_once("._")?.0)
}

fn device_model(instance: &str, deadline: Instant) -> Option<String> {
    let instance = CString::new(instance).ok()?;
    let mut name = [0 as c_char; 1009];
    let mut model: Option<String> = None;
    let mut query = std::ptr::null_mut();
    unsafe {
        if DNSServiceConstructFullName(
            name.as_mut_ptr(),
            instance.as_ptr(),
            c"_device-info._tcp.".as_ptr(),
            c"local.".as_ptr(),
        ) != 0
            || DNSServiceQueryRecord(
                &mut query,
                0,
                0,
                name.as_ptr(),
                16,
                1,
                txt,
                (&raw mut model).cast(),
            ) != 0
        {
            return None;
        }
        let mut poll = libc::pollfd {
            fd: DNSServiceRefSockFD(query),
            events: libc::POLLIN,
            revents: 0,
        };
        while model.is_none() {
            let left = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as i32;
            if left == 0
                || libc::poll(&mut poll, 1, left) <= 0
                || DNSServiceProcessResult(query) != 0
            {
                break;
            }
        }
        DNSServiceRefDeallocate(query);
    }
    model
}

unsafe extern "C" fn txt(
    _: *mut c_void,
    _: u32,
    _: u32,
    error: i32,
    _: *const c_char,
    _: u16,
    _: u16,
    size: u16,
    record: *const c_void,
    _: u32,
    model: *mut c_void,
) {
    let mut length = 0;
    let value = unsafe { TXTRecordGetValuePtr(size, record, c"model".as_ptr(), &mut length) };
    if error == 0 && !value.is_null() {
        let value = unsafe { std::slice::from_raw_parts(value.cast::<u8>(), length.into()) };
        unsafe { *model.cast::<Option<String>>() = Some(String::from_utf8_lossy(value).into()) };
    }
}

/// The SF Symbol Finder's sidebar draws for a server of this model.
pub fn symbol(model: Option<&str>) -> String {
    let tag = NSString::from_str("com.apple.device-model-code");
    let kind = model
        .and_then(|model| {
            UTType::typeWithTag_tagClass_conformingToType(&NSString::from_str(model), &tag, None)
        })
        .filter(|kind| !kind.isDynamic())
        .or_else(|| UTType::typeWithIdentifier(&NSString::from_str("com.apple.mac")));
    let Some(kind) = kind else {
        return "display".into();
    };
    type_symbol(&kind).unwrap_or_else(|| {
        let family = FAMILIES.iter().find(|(family, _)| {
            UTType::typeWithIdentifier(&NSString::from_str(family))
                .is_some_and(|family| kind.conformsToType(&family))
        });
        family.map_or("display", |(_, symbol)| symbol).into()
    })
}

/// IconServices' symbol for a type is the one Finder draws, or a document where it has none.
fn type_symbol(kind: &UTType) -> Option<String> {
    unsafe {
        libc::dlopen(
            c"/System/Library/PrivateFrameworks/IconServices.framework/IconServices".as_ptr(),
            libc::RTLD_LAZY,
        )
    };
    let class = AnyClass::get(c"ISTypeIcon").filter(|class| {
        class.responds_to(sel!(initWithType:)) && class.responds_to(sel!(symbol))
    })?;
    let icon: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    let icon: Option<Retained<AnyObject>> =
        unsafe { msg_send![icon, initWithType: &*kind.identifier()] };
    let symbol: Option<Retained<AnyObject>> = unsafe { msg_send![&*icon?, symbol] };
    let name: Option<Retained<NSString>> = match symbol {
        Some(symbol) if symbol.class().responds_to(sel!(name)) => unsafe {
            msg_send![&symbol, name]
        },
        _ => None,
    };
    Some(name.map_or("doc".into(), |name| name.to_string()))
}

#[repr(C)]
struct Context {
    version: isize,
    info: *const c_void,
    retain: unsafe extern "C" fn(*const c_void) -> *const c_void,
    release: unsafe extern "C" fn(*const c_void),
    describe: *const c_void,
}

type Browsed =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, isize, isize, *mut c_void);

struct Sharing {
    create: unsafe extern "C" fn(*const c_void, *const NSString) -> *mut c_void,
    set_client: unsafe extern "C" fn(*mut c_void, Browsed, *const Context),
    set_queue: unsafe extern "C" fn(*mut c_void, *mut c_void),
    open: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, isize),
    children: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut NSArray<AnyObject>,
    invalidate: unsafe extern "C" fn(*mut c_void),
    url: unsafe extern "C" fn(*const AnyObject) -> *mut NSURL,
    model: unsafe extern "C" fn(*const AnyObject) -> *mut NSString,
    network: *const *const NSString,
}

impl Sharing {
    fn load() -> Option<Self> {
        let path = c"/System/Library/PrivateFrameworks/Sharing.framework/Sharing";
        let lib = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        fn find<T>(lib: *mut c_void, name: &CStr) -> Option<T> {
            let found = unsafe { libc::dlsym(lib, name.as_ptr()) };
            (!lib.is_null() && !found.is_null())
                .then(|| unsafe { std::mem::transmute_copy(&found) })
        }
        Some(Sharing {
            create: find(lib, c"SFBrowserCreate")?,
            set_client: find(lib, c"SFBrowserSetClient")?,
            set_queue: find(lib, c"SFBrowserSetDispatchQueue")?,
            open: find(lib, c"SFBrowserOpenNode")?,
            children: find(lib, c"SFBrowserCopyChildren")?,
            invalidate: find(lib, c"SFBrowserInvalidate")?,
            url: find(lib, c"SFNodeCopyURL")?,
            model: find(lib, c"SFNodeCopyModel")?,
            network: find(lib, c"kSFBrowserKindNetwork")?,
        })
    }
}

type Shared = (Sender<(String, Said)>, Sharing);

fn browse(tx: Sender<(String, Said)>) -> Option<(*mut c_void, *mut c_void, Arc<Shared>)> {
    let shared = Arc::new((tx, Sharing::load()?));
    let sharing = &shared.1;
    let context = Context {
        version: 0,
        info: Arc::as_ptr(&shared).cast(),
        retain,
        release,
        describe: std::ptr::null(),
    };
    unsafe {
        let browser = (sharing.create)(std::ptr::null(), *sharing.network);
        if browser.is_null() {
            return None;
        }
        let queue = dispatch_queue_create(c"finder-fs.servers".as_ptr(), std::ptr::null());
        (sharing.set_client)(browser, browsed, &context);
        (sharing.set_queue)(browser, queue);
        (sharing.open)(browser, std::ptr::null_mut(), std::ptr::null(), 0);
        Some((browser, queue, shared))
    }
}

unsafe extern "C" fn retain(info: *const c_void) -> *const c_void {
    unsafe { Arc::increment_strong_count(info.cast::<Shared>()) };
    info
}

unsafe extern "C" fn release(info: *const c_void) {
    unsafe { Arc::decrement_strong_count(info.cast::<Shared>()) };
}

/// A server whose model sharingd has yet to fetch is listed without one, as is a Bonjour server
/// that has none.
unsafe extern "C" fn browsed(
    browser: *mut c_void,
    node: *mut c_void,
    _: *const c_void,
    _: isize,
    _: isize,
    info: *mut c_void,
) {
    let (tx, sharing) = unsafe { &*info.cast::<Shared>() };
    let children = unsafe { Retained::from_raw((sharing.children)(browser, node)) };
    for child in children.iter().flat_map(|children| children.iter()) {
        let url = unsafe { Retained::from_raw((sharing.url)(&*child)) };
        let model = unsafe { Retained::from_raw((sharing.model)(&*child)) };
        if let Some(host) = url.and_then(|url| url.host()) {
            let said = model.map_or(Said::Unknown, |model| Said::Model(model.to_string()));
            tx.send((host.to_string(), said)).ok();
        }
    }
}

extern "C" {
    fn CFRelease(object: *const c_void);
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_sync_f(
        queue: *mut c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_release(object: *mut c_void);
    fn DNSServiceConstructFullName(
        full: *mut c_char,
        service: *const c_char,
        kind: *const c_char,
        domain: *const c_char,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn DNSServiceQueryRecord(
        query: *mut *mut c_void,
        flags: u32,
        interface: u32,
        name: *const c_char,
        kind: u16,
        class: u16,
        reply: unsafe extern "C" fn(
            *mut c_void,
            u32,
            u32,
            i32,
            *const c_char,
            u16,
            u16,
            u16,
            *const c_void,
            u32,
            *mut c_void,
        ),
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(query: *mut c_void) -> i32;
    fn DNSServiceProcessResult(query: *mut c_void) -> i32;
    fn DNSServiceRefDeallocate(query: *mut c_void);
    fn TXTRecordGetValuePtr(
        size: u16,
        record: *const c_void,
        key: *const c_char,
        length: *mut u8,
    ) -> *const c_void;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bonjour_mount_source_is_asked_by_its_decoded_name() {
        let name = named("NG%20Jace%E2%80%99s%20Mini._smb._tcp.local");
        assert_eq!(name, "NG Jace’s Mini._smb._tcp.local");
        assert_eq!(instance(&name), Some("NG Jace’s Mini"));
        assert_eq!(instance(&named("ng-mini.local")), None);
        assert_eq!(named("100%"), "100%");
    }
}
