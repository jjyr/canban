# Canban Website (`canban.dev`)

Static product site for [https://canban.dev/](https://canban.dev/), deployed via GitHub Pages (`.github/workflows/pages.yml`). The Python standard-library generator builds 55 complete HTML pages (11 routes across 5 languages: English `/`, Simplified Chinese `/zh/`, Traditional Chinese `/zh-tw/`, Japanese `/ja/`, and Korean `/ko/`) plus 55 Markdown mirrors: JavaScript enhances the CRT product demo, dropdown closing, and copy buttons, but is not required to read localized content, switch languages via `<details class="lang-menu">`, browse installation instructions, FAQs, guides, or release history.

## Pages and content

- `/` (and `/zh/`, `/zh-tw/`, `/ja/`, `/ko/`) retain the four-channel CRT product deck and expose visible navigation links in `CH.01` (`Install`, `Docs`, `Workflows`, `CLI`, `Changelog`) plus guide links in `CH.04` without requiring channel switching. On phones (720px and below), the channels form one vertically scrolling page. Desktop channel navigation and the three-scene product demo remain available.
- `/install/` (and localized equivalents) contain desktop/headless installation, prerequisites, Linux runtime dependencies, updates, source installation links, and six visible FAQs. Agent support follows Herdr support.
- `/docs/` (plus `/docs/workflows/`, `/docs/cli/`, `/docs/agents/`, `/docs/remote-hosts/`) and the three scenario guides (`/guides/parallel-coding-agents/`, `/guides/native-terminal-and-multi-machine/`, and `/guides/scheduled-worktree-cleanup/`) provide architecture and Privacy & Security (`#privacy-security`) guides, Timeline-first Insights, verified `schema: 13` Workflow YAML syntax (including `default_agent`/`override`, cardless GitHub events, and `XChaCha20-Poly1305` encrypted secrets), the Agent-first `canban` CLI and `--human` guide, built-in CLI agent catalog with managed terminal presence leases & `agy` Orchestrator config, remote Linux `canband` (direct `Noise_XX` E2EE `yamux/1` TCP with X25519 public-key auth & host key pinning, optional Tailscale WhoIs & SSH, and offline editing with 3-way sync) guides, and visible FAQs with `TechArticle`, `BreadcrumbList`, and `FAQPage` JSON-LD.
- `/changelog/` shows published GitHub Releases in descending version order (`YYYYMMDD-N`, comparing the daily sequence numerically), preserving the original release-note language. Non-English `/changelog/` routes remain accessible for in-site navigation, carry `<meta name="robots" content="noindex, follow">`, omit cross-language `hreflang` tags, and are excluded from `sitemap.xml` so duplicate English release notes do not compete with `/changelog/`. Historical prereleases are labeled; drafts and records without a publication date are excluded.
- All 50 non-changelog HTML routes have self-referencing canonical URLs, localized metadata, `<link rel="alternate" type="text/markdown">` mirrors, and reciprocal `en`, `zh-CN`, `zh-TW`, `ja`, `ko`, and `x-default` alternates. `sitemap.xml` includes 51 canonical URLs (all 50 non-changelog localized pages plus English `/changelog/`) with page-scoped `<lastmod>` dates. Legacy `?lang=en|zh|zh-CN|zh-TW|zh-HK|ja|ko` links redirect in the browser while preserving other parameters and channel hashes. Browser preferences do not override the URL's language.
- The generator also builds `/llms-full.txt` at site root combining disambiguation context, user docs, scenario guides, workflow syntax/semantics, CLI reference, FAQs, and `skills/canban/SKILL.md`, alongside `llms.txt`.
- Every page embeds localized `WebSite` and `SoftwareApplication` JSON-LD driven by `schema.json["localized"]`.

## Source structure

- `templates/home.html` — shared homepage body with `data-i18n` keys, `{{lang_switcher}}`, and resource URL placeholders.
- `locales.json` — 5-language homepage translations, metadata, and per-locale `updated` dates.
- `pages.json` — 5-language installation, FAQ, navigation, and release-page translations.
- `docs/<lang>.json` (`en.json`, `zh.json`, `zh-tw.json`, `ja.json`, `ko.json`) — per-language content, FAQs, and publication/update dates for `/docs/*` and `/guides/*`.
- `schema.json` — shared site/software graph plus per-language `localized` properties.
- `styles.css` — original Monokai CRT design and `<details class="lang-menu">` dropdown styling; `pages.css` — document layout, tables, copyable code blocks, and responsive corrections.
- `app.js` — 5-language demo scenes, desktop channel navigation, installation-mode selection, language-menu close behavior, and copy helpers. Copy labels are embedded in each generated page; content language is never rewritten at runtime.
- `releases.json` — checked-in published-release snapshot for deterministic offline builds. Refresh it intentionally when updating the snapshot; deployments fetch current releases independently.
- `../scripts/website.py` — generator, release fetching/filtering, conservative escaped Markdown rendering (including tables and copyable code blocks for trusted docs), `.md` mirror emitter, and `llms-full.txt` bundler.
- `install.sh` — source compatibility symlink. The generator copies the canonical `../scripts/install.sh` into the output.
- `CNAME`, icons, `og-image.png`, `screenshots/`, `site.webmanifest`, `robots.txt`, `llms.txt` — public assets. Templates and content JSON are not deployed.

## Build and preview

```sh
mise run build-website
mise run test-website
python3 -m http.server 8080 --bind 127.0.0.1 --directory .local/website
```

Open `http://127.0.0.1:8080/`, `/zh/`, `/zh-tw/`, `/ja/`, `/ko/`, `/guides/parallel-coding-agents/`, `/guides/native-terminal-and-multi-machine/`, `/guides/scheduled-worktree-cleanup/`, `/docs/`, or `/changelog/`. Preview the generated directory, not the source directory. No Node package installation or frontend framework is required to build the site.

Fetch current release history during a build with authenticated GitHub CLI access:

```sh
mise run build-website -- --refresh-releases
```

This paginates the repository's GitHub Releases API, filters unpublished records and exposes only public release fields. An API failure fails the build rather than publishing empty or stale release history. The source snapshot is not rewritten by a build.

## Deployment and verification

The Pages workflow validates the generator, refreshes release history, and uploads only `.local/website`. It runs on relevant `main` pushes, manual dispatch, published-release changes, and successful completion of the `Release` workflow. The completion trigger also covers releases created using `GITHUB_TOKEN`, whose release event does not start another workflow. Release-triggered builds use the default branch's website source.

`mise run test-website` checks complete localized HTML and metadata across all 55 pages (5 languages × 11 routes), visible `CH.01` navigation links, absence of removed `/workflows/` and `/compare/` routes across all language prefixes, visible FAQ content and `FAQPage` JSON-LD on install/docs/guides, localized `SoftwareApplication` Schema, enriched `TechArticle` and `BreadcrumbList` JSON-LD, non-English `/changelog/` `noindex` and `sitemap.xml` exclusion, page-scoped `lastmod`, `.md` mirrors, `llms.txt` / `llms-full.txt`, 24-CLI catalog parity with `crates/profiles/src/catalog.rs`, local links/assets/fragments, all sitemap language pairs, canonical installer equality, release coverage, draft exclusion, and escaping of untrusted release notes and doc Markdown. Browser acceptance must additionally cover multiple languages and viewports (`320`, `390`, `720`, `768`, `1280px`), `<details class="lang-menu">` language switching, desktop channel navigation, mobile scrolling, installation tabs, actual clipboard copying, legacy query redirects, and JavaScript-disabled rendering. Rust/native app tests do not exercise this standalone website.

## License

Apache-2.0 · Copyright 2026 JJy <jjyruby@gmail.com>.
