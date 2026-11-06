use nsh_core::element::{AdaptiveColor, rgb};
const fn tone(light: u32, dark: u32) -> AdaptiveColor {
    AdaptiveColor {
        light: rgb(light).to_hsla(),
        dark: rgb(dark).to_hsla(),
    }
}
pub const BACKGROUND: AdaptiveColor = tone(0xf5f6f8, 0x101318);
pub const SURFACE: AdaptiveColor = tone(0xffffff, 0x191e26);
pub const INSET: AdaptiveColor = tone(0xedeff3, 0x13171d);
pub const TEXT: AdaptiveColor = tone(0x1b2533, 0xe5eaf2);
pub const MUTED: AdaptiveColor = tone(0x657185, 0x9aa8bb);
pub const BORDER: AdaptiveColor = tone(0xdce1e9, 0x303946);
pub const ACCENT: AdaptiveColor = tone(0x285bca, 0x80aaff);
pub const ACCENT_FILL: AdaptiveColor = tone(0x285bca, 0x315faa);
pub const ON_ACCENT: AdaptiveColor = tone(0xffffff, 0xffffff);
pub const DANGER: AdaptiveColor = tone(0xad3c36, 0xffa39c);
pub const SUCCESS: AdaptiveColor = tone(0x27745c, 0x7dd6b5);
pub const RADIUS: f32 = 8.0;
pub const CONTROL_HEIGHT: f32 = 34.0;
