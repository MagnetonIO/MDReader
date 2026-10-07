//! MDReader native provider kernel, with temporary legacy app compatibility.
#[cfg(feature="legacy")]
mod legacy;
#[cfg(feature="legacy")]
pub use legacy::*;
pub mod native;
