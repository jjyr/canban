//! Canban's shared visual layer. No product/domain or service dependencies.
pub mod button;
pub mod checkbox;
pub mod input;
pub mod surface;
pub mod tokens;
pub use button::{Variant, action};
pub use checkbox::checkbox;
pub use input::{bound_field, field};
pub use surface::{badge, heading, label, muted, panel, read_text};

pub mod navigation;
pub mod split;
pub mod status;
pub use navigation::navigation;
pub use split::split;
pub use status::{Tone, notice, status};
