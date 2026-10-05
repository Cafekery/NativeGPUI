use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSString, NSURL, ns_string};
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
    /// sharingd lists a server without its model until it has fetched one.
    Unknown,
    Unrecorded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Computer {
    pub name: String,
    pub model: Option<String>,
    pub protocols: Vec<String>,
}

impl Computer {
    pub fn url(&self) -> Option<String> {
        let (scheme, service) = [
            ("smb", "_smb._tcp"),
            ("afp", "_afpovertcp._tcp"),
            ("vnc", "_rfb._tcp"),
        ]
        .into_iter()
        .find(|(scheme, _)| self.protocols.iter().any(|offered| offered == scheme))?;
        Some(format!(
            "{scheme}://{}.{service}.local",
            escaped(&self.name)
        ))
    }
}

fn escaped(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for byte in name.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                escaped.push(char::from(byte))
            }
            _ => write!(escaped, "%{byte:02X}").unwrap(),
        }
    }
    escaped
}

/// Finder's Network list: `listed` gets the whole list each time it changes, until it returns false.
pub fn network(mut listed: impl FnMut(Vec<Computer>) -> bool) {
    let (tx, lists) = channel();
    let Some(_browsing) = browse(tx, networked) else {
        return;
    };
    while let Ok(list) = lists.recv() {
        if !autoreleasepool(|_| listed(list)) {
            break;
        }
    }
}

/// `found` gets each host's model as it lands within `wait`; `None` is a Bonjour host without one.
pub fn models(hosts: &[String], wait: Duration, mut found: impl FnMut(&str, Option<&str>)) {
    let deadline = Instant::now() + wait;
    let (tx, answers) = channel();
    let _browsing = browse(tx.clone(), browsed);
    let mut left: Vec<(String, &String, [bool; 2])> = hosts
        .iter()
        .map(|host| (named(host), host, [false; 2]))
        .collect();
    // Bonjour answers `_device-info` in milliseconds; sharingd can take a minute after a mount.
    for (name, ..) in &left {
        let Some(instance) = instance(name).map(str::to_owned) else {
            continue;
        };
        let (tx, name) = (tx.clone(), name.clone());
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
}

pub(crate) fn title(host: &str) -> String {
    let name = named(host);
    instance(&name).map(str::to_owned).unwrap_or(name)
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

fn device_type(model: Option<&str>) -> Option<Retained<UTType>> {
    let tag = ns_string!("com.apple.device-model-code");
    model
        .and_then(|model| {
            UTType::typeWithTag_tagClass_conformingToType(&NSString::from_str(model), tag, None)
        })
        .filter(|kind| !kind.isDynamic())
        .or_else(|| UTType::typeWithIdentifier(ns_string!("com.apple.mac")))
}

/// The type identifier and kind name of a computer of this model.
pub fn model_type(model: Option<&str>) -> Option<(String, String)> {
    let kind = device_type(model)?;
    let named = kind.localizedDescription().map(|named| named.to_string());
    Some((kind.identifier().to_string(), named.unwrap_or_default()))
}

pub fn symbol(model: Option<&str>) -> String {
    let Some(kind) = device_type(model) else {
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

fn icon_class(name: &CStr, init: Sel) -> Option<&'static AnyClass> {
    unsafe {
        libc::dlopen(
            c"/System/Library/PrivateFrameworks/IconServices.framework/IconServices".as_ptr(),
            libc::RTLD_LAZY,
        )
    };
    AnyClass::get(name).filter(|class| class.responds_to(init) && class.responds_to(sel!(symbol)))
}

fn symbol_name(icon: &AnyObject) -> Option<String> {
    let symbol: Option<Retained<AnyObject>> = unsafe { msg_send![icon, symbol] };
    let symbol = symbol.filter(|symbol| symbol.class().responds_to(sel!(name)))?;
    let name: Option<Retained<NSString>> = unsafe { msg_send![&symbol, name] };
    name.map(|name| name.to_string())
}

/// IconServices' symbol for a type is the one Finder draws.
fn type_symbol(kind: &UTType) -> Option<String> {
    let class = icon_class(c"ISTypeIcon", sel!(initWithType:))?;
    let icon: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    let icon: Option<Retained<AnyObject>> =
        unsafe { msg_send![icon, initWithType: &*kind.identifier()] };
    Some(symbol_name(&*icon?).unwrap_or_else(|| "doc".into()))
}

pub(crate) fn bundle_symbol(identifier: &str) -> Option<String> {
    let class = icon_class(c"ISBundleIdentifierIcon", sel!(initWithBundleIdentifier:))?;
    let icon: Allocated<AnyObject> = unsafe { msg_send![class, alloc] };
    let icon: Option<Retained<AnyObject>> =
        unsafe { msg_send![icon, initWithBundleIdentifier: &*NSString::from_str(identifier)] };
    symbol_name(&*icon?)
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
    name: unsafe extern "C" fn(*const AnyObject) -> *mut NSString,
    protocols: unsafe extern "C" fn(*const AnyObject) -> *mut NSArray<NSString>,
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
            name: find(lib, c"SFNodeCopyDisplayName")?,
            protocols: find(lib, c"SFNodeCopyProtocols")?,
            network: find(lib, c"kSFBrowserKindNetwork")?,
        })
    }
}

struct Browsing<T> {
    browser: *mut c_void,
    queue: *mut c_void,
    shared: Arc<(T, Sharing)>,
}

impl<T> Drop for Browsing<T> {
    fn drop(&mut self) {
        unsafe {
            dispatch_sync_f(self.queue, self.browser, self.shared.1.invalidate);
            CFRelease(self.browser);
            dispatch_release(self.queue);
        }
    }
}

fn browse<T>(state: T, client: Browsed) -> Option<Browsing<T>> {
    let shared = Arc::new((state, Sharing::load()?));
    let sharing = &shared.1;
    let context = Context {
        version: 0,
        info: Arc::as_ptr(&shared).cast(),
        retain: retain::<T>,
        release: release::<T>,
        describe: std::ptr::null(),
    };
    unsafe {
        let browser = (sharing.create)(std::ptr::null(), *sharing.network);
        if browser.is_null() {
            return None;
        }
        let queue = dispatch_queue_create(c"finder-fs.servers".as_ptr(), std::ptr::null());
        (sharing.set_client)(browser, client, &context);
        (sharing.set_queue)(browser, queue);
        (sharing.open)(browser, std::ptr::null_mut(), std::ptr::null(), 0);
        Some(Browsing {
            browser,
            queue,
            shared,
        })
    }
}

unsafe extern "C" fn retain<T>(info: *const c_void) -> *const c_void {
    unsafe { Arc::increment_strong_count(info.cast::<(T, Sharing)>()) };
    info
}

unsafe extern "C" fn release<T>(info: *const c_void) {
    unsafe { Arc::decrement_strong_count(info.cast::<(T, Sharing)>()) };
}

unsafe extern "C" fn browsed(
    browser: *mut c_void,
    node: *mut c_void,
    _: *const c_void,
    _: isize,
    _: isize,
    info: *mut c_void,
) {
    let (tx, sharing) = unsafe { &*info.cast::<(Sender<(String, Said)>, Sharing)>() };
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

unsafe extern "C" fn networked(
    browser: *mut c_void,
    node: *mut c_void,
    _: *const c_void,
    _: isize,
    _: isize,
    info: *mut c_void,
) {
    let (tx, sharing) = unsafe { &*info.cast::<(Sender<Vec<Computer>>, Sharing)>() };
    let children = unsafe { Retained::from_raw((sharing.children)(browser, node)) };
    let list = children
        .iter()
        .flat_map(|children| children.iter())
        .filter_map(|child| {
            let name = unsafe { Retained::from_raw((sharing.name)(&*child)) }?;
            let model = unsafe { Retained::from_raw((sharing.model)(&*child)) };
            let protocols = unsafe { Retained::from_raw((sharing.protocols)(&*child)) };
            Some(Computer {
                name: name.to_string(),
                model: model.map(|model| model.to_string()),
                protocols: protocols
                    .iter()
                    .flat_map(|protocols| protocols.iter())
                    .map(|protocol| protocol.to_string())
                    .collect(),
            })
        })
        .collect();
    tx.send(list).ok();
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

    #[test]
    fn a_computer_opens_its_files_before_its_screen_and_mounts_under_its_own_name() {
        let url = |protocols: &[&str]| {
            let protocols = protocols.iter().map(|it| it.to_string()).collect();
            let name = "Jace’s MacBook Air".into();
            Computer {
                name,
                model: None,
                protocols,
            }
            .url()
        };
        let smb = url(&["vnc", "afp", "smb"]).unwrap();
        assert_eq!(smb, "smb://Jace%E2%80%99s%20MacBook%20Air._smb._tcp.local");
        assert_eq!(
            title(smb.strip_prefix("smb://").unwrap()),
            "Jace’s MacBook Air"
        );
        assert!(url(&["vnc", "afp"]).unwrap().starts_with("afp://"));
        assert!(url(&["afp"]).unwrap().ends_with("._afpovertcp._tcp.local"));
        assert!(url(&["vnc"]).unwrap().ends_with("._rfb._tcp.local"));
        assert_eq!(url(&[]), None);
    }
}
