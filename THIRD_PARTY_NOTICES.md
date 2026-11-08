# Third-party notices

Canban is Copyright 2026 JJy <jjyruby@gmail.com>, Apache-2.0.

- The terminal build integration and VT/frame/input implementation reference herdr-app e77f112ac2ea136b84fe510efa42d41163bfd776, Copyright its respective contributors, Apache-2.0. The source is https://github.com/herdrdev/herdr-app. See third_party/Herdr-LICENSE.txt. GPUI renderer code is not linked into Canban.
- libghostty-vt is built from ghostty-org/ghostty 5813c9461d5af78b6307a1254d00b1bc190eaccd, Copyright 2024 Mitchell Hashimoto and Ghostty contributors, MIT. See third_party/Ghostty-LICENSE.txt. The commit-verified upstream source checkout, including its applicable notices, is retained under .local/deps/ghostty for the build; unused GUI resources are excluded.
- The vendored UI icons in crates/ui-kit/assets/lucide are from Lucide 0.468.0 (https://github.com/lucide-icons/lucide/tree/0.468.0/icons), ISC. Copyright Lucide Contributors 2022; portions derived from Feather are Copyright Cole Bemis 2013–2022 (MIT). See third_party/Lucide-LICENSE.txt and the preserved upstream assets/lucide/LICENSE. Their original 24-unit geometry and 2-unit stroke render at 20 points in Canban.

NSH and other Cargo dependencies retain their upstream licenses. No third-party attribution replaces Canban's Apache-2.0 license.
