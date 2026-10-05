use std::sync::LazyLock;

use gpui::{AnyElement, Pixels, div, prelude::*};
use native_gpui::scroll_edge_effect;
use objc2_foundation::NSProcessInfo;

static MACOS_27: LazyLock<bool> = LazyLock::new(|| {
    NSProcessInfo::processInfo()
        .operatingSystemVersion()
        .majorVersion
        >= 27
});

pub(crate) fn toolbar_rule() -> bool {
    !*MACOS_27
}

pub(crate) fn band_edge(band: Pixels, header: Pixels) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .child(scroll_edge_effect("ng.finder.band").header(header))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(band)
                .block_mouse_except_scroll(),
        )
        .into_any_element()
}
