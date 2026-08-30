//! Vibrancy and macOS 26 liquid glass. Put these behind or over your GPUI layout.

#[cfg(feature = "private-api")]
mod custom_glass;
mod glass;
#[cfg(feature = "private-api")]
mod glass_private;
mod material;
#[cfg(feature = "private-api")]
pub(crate) mod private_api;

#[cfg(feature = "private-api")]
pub use custom_glass::{
    CustomGlass, GlassRecipe, INERT_INPUTS, custom_glass, custom_glass_available,
};
pub use glass::{Glass, GlassContainer, glass, glass_container};
#[cfg(feature = "private-api")]
pub use glass_private::{GlassInteractionState, GlassSubvariant, GlassVariant};
pub use material::{Blending, Material, NativeMaterial, RawMaterial, material};
