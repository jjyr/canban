//! Built-in named dark themes, shared by preference validation and renderers.
//! Classic palette provenance: ../README.md and third_party/iTerm2-Color-Schemes-LICENSE.txt.
#[derive(Debug)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub ansi: [u32; 16],
}

pub const DEFAULT: &str = "canban";
pub static THEMES: &[Theme] = &[
    Theme {
        id: "canban",
        name: "Canban",
        background: 0x191e26,
        foreground: 0xe5eaf2,
        cursor: 0x80aaff,
        ansi: [
            0x1b2533, 0xbe4040, 0x378a5b, 0xb58b23, 0x80aaff, 0x975bb4, 0x2d8c97, 0xdbe1e9,
            0x6a7689, 0xf17e7e, 0x82cfa1, 0xebcc77, 0x80aaff, 0xc8a0e7, 0x83cdd4, 0xf4f6f9,
        ],
    },
    Theme {
        id: "dracula",
        name: "Dracula",
        background: 0x282a36,
        foreground: 0xf8f8f2,
        cursor: 0xf8f8f2,
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    },
    Theme {
        id: "nord",
        name: "Nord",
        background: 0x2e3440,
        foreground: 0xd8dee9,
        cursor: 0xeceff4,
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x596377, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    Theme {
        id: "gruvbox",
        name: "Gruvbox",
        background: 0x282828,
        foreground: 0xebdbb2,
        cursor: 0xebdbb2,
        ansi: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    },
    Theme {
        id: "tokyo-night",
        name: "Tokyo Night",
        background: 0x1a1b26,
        foreground: 0xc0caf5,
        cursor: 0xc0caf5,
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    Theme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        background: 0x1e1e2e,
        foreground: 0xcdd6f4,
        cursor: 0xf5e0dc,
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf7aec2, 0xc2ecbf, 0xfcd682, 0xaeccfc, 0xf398da, 0xb1eae1, 0xa6adc8,
        ],
    },
    Theme {
        id: "one-dark",
        name: "One Dark",
        background: 0x21252b,
        foreground: 0xabb2bf,
        cursor: 0xabb2bf,
        ansi: [
            0x21252b, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x767676, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        ],
    },
    Theme {
        id: "monokai",
        name: "Monokai",
        background: 0x272822,
        foreground: 0xfdfff1,
        cursor: 0xc0c1b5,
        ansi: [
            0x272822, 0xf92672, 0xa6e22e, 0xe6db74, 0xfd971f, 0xae81ff, 0x66d9ef, 0xfdfff1,
            0x6e7066, 0xf92672, 0xa6e22e, 0xe6db74, 0xfd971f, 0xae81ff, 0x66d9ef, 0xfdfff1,
        ],
    },
    Theme {
        id: "solarized",
        name: "Solarized",
        background: 0x002b36,
        foreground: 0x839496,
        cursor: 0x839496,
        ansi: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x335e69, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    },
    Theme {
        id: "tomorrow-night",
        name: "Tomorrow Night",
        background: 0x1d1f21,
        foreground: 0xc5c8c6,
        cursor: 0xc5c8c6,
        ansi: [
            0x000000, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xffffff,
            0x4c4c4c, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xffffff,
        ],
    },
    Theme {
        id: "oceanic-next",
        name: "Oceanic Next",
        background: 0x162c35,
        foreground: 0xc0c5ce,
        cursor: 0xc0c5ce,
        ansi: [
            0x162c35, 0xec5f67, 0x99c794, 0xfac863, 0x6699cc, 0xc594c5, 0x5fb3b3, 0xffffff,
            0x65737e, 0xec5f67, 0x99c794, 0xfac863, 0x6699cc, 0xc594c5, 0x5fb3b3, 0xffffff,
        ],
    },
    Theme {
        id: "material",
        name: "Material",
        background: 0x232322,
        foreground: 0xe5e5e5,
        cursor: 0x16afca,
        ansi: [
            0x212121, 0xb7141f, 0x457b24, 0xf6981e, 0x134eb2, 0x701aa2, 0x0e717c, 0xefefef,
            0x4f4f4f, 0xe83b3f, 0x7aba3a, 0xffea2e, 0x54a4f3, 0xaa4dbc, 0x26bbd1, 0xd9d9d9,
        ],
    },
    Theme {
        id: "ayu",
        name: "Ayu",
        background: 0x0b0e14,
        foreground: 0xbfbdb6,
        cursor: 0xe6b450,
        ansi: [
            0x11151c, 0xea6c73, 0x7fd962, 0xf9af4f, 0x53bdfa, 0xcda1fa, 0x90e1c6, 0xc7c7c7,
            0x686868, 0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xffffff,
        ],
    },
    Theme {
        id: "everforest",
        name: "Everforest",
        background: 0x232a2e,
        foreground: 0xd3c6aa,
        cursor: 0xe69875,
        ansi: [
            0x7a8478, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xf2efdf,
            0xa6b0a0, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xfffbef,
        ],
    },
    Theme {
        id: "rose-pine",
        name: "Rosé Pine",
        background: 0x191724,
        foreground: 0xe0def4,
        cursor: 0xe0def4,
        ansi: [
            0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
            0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
        ],
    },
    Theme {
        id: "kanagawa",
        name: "Kanagawa",
        background: 0x1f1f28,
        foreground: 0xdcd7ba,
        cursor: 0xdcd7ba,
        ansi: [
            0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
            0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
        ],
    },
    Theme {
        id: "ayu-mirage",
        name: "Ayu Mirage",
        background: 0x1f2430,
        foreground: 0xcccac2,
        cursor: 0xffcc66,
        ansi: [
            0x171b24, 0xed8274, 0x87d96c, 0xfacc6e, 0x6dcbfa, 0xdabafa, 0x90e1c6, 0xc7c7c7,
            0x686868, 0xf28779, 0xd5ff80, 0xffd173, 0x73d0ff, 0xdfbfff, 0x95e6cb, 0xffffff,
        ],
    },
];

pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|theme| theme.id == id)
}
pub fn resolve(id: &str) -> &'static Theme {
    find(id).unwrap_or(&THEMES[0])
}
/// Semantic chrome colors: background, surface, inset, text, muted, border,
/// accent, accent fill, on-accent, danger, success, selected, attention and tints.
impl Theme {
    /// A readable purple/magenta highlight distinct from success and runtime activity.
    pub fn highlight(&self) -> (u32, u32) {
        let foreground = readable(self.ansi[13], self.background);
        let surface = mix(self.background, foreground, 0.12);
        (surface, readable(foreground, surface))
    }

    pub fn chrome(&self) -> [u32; 16] {
        if self.id == DEFAULT {
            return [
                0x101318, 0x191e26, 0x13171d, 0xe5eaf2, 0x9aa8bb, 0x303946, 0x80aaff, 0x315faa,
                0xffffff, 0xffa39c, 0x7dd6b5, 0x26364e, 0xf0c679, 0x3a3020, 0x20392f, 0x402a2a,
            ];
        }
        let bg = self.background;
        let fg = readable(self.foreground, bg);
        let accent = readable(self.ansi[12], bg);
        let danger = readable(self.ansi[9], bg);
        let success = readable(self.ansi[10], bg);
        let attention = readable(self.ansi[11], bg);
        let fill = mix(bg, accent, 0.35);
        [
            mix(bg, 0, 0.2),
            bg,
            mix(bg, 0, 0.12),
            fg,
            readable(mix(bg, fg, 0.7), bg),
            mix(bg, fg, 0.2),
            accent,
            fill,
            readable(0xffffff, fill),
            danger,
            success,
            mix(bg, accent, 0.15),
            attention,
            mix(bg, attention, 0.1),
            mix(bg, success, 0.1),
            mix(bg, danger, 0.1),
        ]
    }
}
fn mix(a: u32, b: u32, weight: f64) -> u32 {
    [16, 8, 0].into_iter().fold(0, |color, shift| {
        let a = ((a >> shift) & 255) as f64;
        let b = ((b >> shift) & 255) as f64;
        color | (((a + (b - a) * weight).round() as u32) << shift)
    })
}
fn luminance(color: u32) -> f64 {
    [16, 8, 0]
        .into_iter()
        .zip([0.2126, 0.7152, 0.0722])
        .map(|(shift, weight)| {
            let c = ((color >> shift) & 255) as f64 / 255.;
            weight
                * if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
        })
        .sum()
}
fn contrast(a: u32, b: u32) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn readable(mut color: u32, background: u32) -> u32 {
    let target = if luminance(background) < 0.18 {
        0xffffff
    } else {
        0
    };
    for _ in 0..64 {
        if contrast(color, background) >= 4.5 {
            return color;
        }
        color = mix(color, target, 0.08);
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_unique_dark_and_readable_with_sixteen_classics_and_canban() {
        assert_eq!(THEMES.len(), 17);
        let mut ids = std::collections::HashSet::new();
        for theme in THEMES {
            assert!(ids.insert(theme.id));
            assert!(
                theme
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c == b'-')
            );
            assert!(luminance(theme.background) < 0.1, "{}", theme.id);
            let chrome = theme.chrome();
            let (highlight_surface, highlight) = theme.highlight();
            assert!(
                contrast(highlight, highlight_surface) >= 4.5,
                "{} highlight",
                theme.id
            );
            for index in [3, 4, 6, 9, 10, 12] {
                assert!(
                    contrast(chrome[index], chrome[1]) >= 4.5,
                    "{} token {index}",
                    theme.id
                );
            }
            assert!(
                contrast(chrome[8], chrome[7]) >= 4.5,
                "{} button text",
                theme.id
            );
        }
        assert_eq!(resolve("missing").id, DEFAULT);
        assert_eq!(THEMES[0].id, DEFAULT);
        // A classic's ANSI blue must not be replaced by the App accent.
        assert_eq!(resolve("dracula").ansi[4], 0xbd93f9);
        assert_eq!(resolve("dracula").background, 0x282a36);
    }
}
