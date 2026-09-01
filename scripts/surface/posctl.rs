// CONTROL for the positive arm — the same shape as pos.rs plus one name that has never existed.
// Must FAIL.
//
// Without it, pos.rs compiling says only that rustc ran. A `--extern` pointed at a stale rlib, a
// path typo that happens to resolve to something else, or an arm that never reached the crate at
// all would all read as a pass.
#![allow(unused_imports)]

use native_gpui::{
    NativeSidebar, element_key, no_such_item_zzz, present_popover, native_switch,
};

fn main() {}
