pub mod complete;
pub mod entry;
pub mod icloud;
pub mod index;
pub mod kind;
pub mod list;
pub mod meta;
pub mod name;
pub mod ops;
pub mod queue;
pub mod servers;
pub mod sidebar;
pub mod sort;
pub mod tags;
pub mod volumes;
pub mod watch;
pub mod words;

pub use entry::{Entry, EntryKind};

/// Walking and watching the disk runs at utility QoS: near full speed, yielding to the
/// foreground. Background QoS measured three times the CPU for the same walk.
pub(crate) fn utility_qos() {
    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
}
