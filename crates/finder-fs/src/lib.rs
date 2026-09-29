pub mod complete;
pub mod ds_store;
pub mod entry;
pub mod folder_icon;
pub mod icloud;
pub mod index;
pub mod kind;
pub mod list;
pub mod meta;
pub mod name;
pub mod ops;
pub mod put_back;
pub mod queue;
pub mod servers;
pub mod sidebar;
pub mod sort;
pub mod tags;
pub mod volumes;
pub mod watch;
pub mod words;

pub use entry::{Entry, EntryKind};

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization, NSString,
};

/// Background QoS measured three times the CPU of utility for the same walk.
pub(crate) fn utility_qos() {
    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
}

pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(crate) fn plist(bytes: &[u8]) -> Option<Retained<AnyObject>> {
    unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(bytes),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .ok()
}

pub(crate) fn plist_dict(bytes: &[u8]) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    let dict = plist(bytes)?.downcast::<NSDictionary>().ok()?;
    // SAFETY: property-list dictionary keys are always strings.
    Some(unsafe { Retained::cast_unchecked(dict) })
}
