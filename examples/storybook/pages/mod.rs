pub mod background;
pub mod buttons;
pub mod chrome;
pub mod glass;
pub mod indicators;
pub mod label;
pub mod material;
pub mod menus;
pub mod popup;
pub mod segmented;
pub mod sidebar_page;
pub mod slider;
pub mod small;
pub mod surfaces;
pub mod switch;
pub mod text;
pub mod tokens;

#[cfg(feature = "private-api")]
pub mod custom_glass;
#[cfg(feature = "private-api")]
pub mod glass_variants;

#[cfg(not(feature = "private-api"))]
pub use needs_private_api as custom_glass;
#[cfg(not(feature = "private-api"))]
pub use needs_private_api as glass_variants;

#[cfg(not(feature = "private-api"))]
pub mod needs_private_api {
    use crate::Demo;
    use crate::ui::{note, stack};
    use gpui::{AnyElement, Context, Window};

    #[derive(Default)]
    pub struct State;

    pub fn render(_: &mut Demo, _: &mut Window, _: &mut Context<Demo>) -> AnyElement {
        stack(vec![note("Run with --features private-api")])
    }
}
