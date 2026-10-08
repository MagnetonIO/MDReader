//! Native wire types for portable device assets.
//! Persistence, validation and read-session ownership live in mdreader-core.
mod types;

pub use mdreader_core::device::assets::AssetRuntime;
pub use types::*;
