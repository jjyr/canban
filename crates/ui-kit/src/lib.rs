//! Canban's shared visual layer. No product/domain or service dependencies.
pub mod button;
pub mod checkbox;
pub mod input;
pub mod scrollbar;
pub mod surface;
pub mod tokens;
pub use button::{Variant, action, action_with_icon, compact_action, press_target};
pub use checkbox::checkbox;
pub use input::{bound_field, field};
pub use scrollbar::vertical_scrollbar;
pub use surface::{badge, heading, label, muted, panel, read_text};

pub mod navigation;
pub mod split;
pub mod status;
pub use navigation::{navigation, subpage_header};
pub use split::split;
pub use status::{
    Tone, attention_count, live_dot, live_status, notice, reminder_count, status, tone_colors,
    tone_icon_badge,
};

pub mod icons;
pub use icons::{Icon, icon, icon_action, icon_colored, icon_tinted};
pub mod search_dialog;
pub use search_dialog::{SearchChoice, search_dialog};
pub mod picker;
pub use picker::{PickerOption, SearchPopover, filter_picker_options, search_popover};

pub mod entry_card;
pub use entry_card::entry_card;
pub mod directory_field;
pub use directory_field::{directory_field, directory_input};
pub mod settings;
pub use settings::{settings_field, settings_nav_heading, settings_nav_item, settings_section};
pub mod section_group;
pub use section_group::{SectionGroup, collapsible_section_group, section_group};
pub mod chrome;
pub use chrome::{
    chrome_action, chrome_button, chrome_button_disabled, chrome_indicator_button, utility_button,
};

pub mod segmented;
pub use segmented::{compact_segment, segment, segmented};

pub mod theme_choice;
pub use theme_choice::theme_choice;

pub mod markdown;

pub mod motion;
pub mod search;
pub mod tree_group;
pub use tree_group::tree_group;

pub mod reference_link;
pub mod tooltip;
pub use tooltip::{hint_tooltip, tooltip_box};
