//! Canban's shared visual layer. No product/domain or service dependencies.
pub mod button;
pub mod checkbox;
pub mod input;
mod scrollbar;
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
pub use status::{Tone, attention_count, notice, status};

pub mod icons;
pub use icons::{Icon, icon, icon_action, icon_tinted};
pub mod search_dialog;
pub use search_dialog::{SearchChoice, search_dialog};

pub mod entry_card;
pub use entry_card::entry_card;
pub mod directory_field;
pub use directory_field::{directory_field, directory_input};
pub mod settings;
pub use settings::{settings_field, settings_section};
pub mod chrome;
pub use chrome::{chrome_button, chrome_button_disabled, utility_button};

pub mod segmented;
pub use segmented::{segment, segmented};

pub mod theme_choice;
pub use theme_choice::theme_choice;
