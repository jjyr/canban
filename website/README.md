# Canban Website (`canban.dev`)

Static product site for [https://canban.dev/](https://canban.dev/), deployed via GitHub Pages (`.github/workflows/pages.yml`). The Python standard-library generator produces complete HTML: JavaScript enhances the product demo and copy buttons, but is not required to read localized content, installation instructions, FAQs, or release history.

## Pages and content

- `/` and `/zh/` retain the four-channel CRT product deck. On phones (720px and below), the channels form one vertically scrolling page. Desktop channel navigation and the three-scene product demo remain available.
- `/install/` and `/zh/install/` contain desktop/headless installation, prerequisites, Linux runtime dependencies, updates, source installation links, and six visible FAQs. Agent support follows Herdr support.
- `/changelog/` and `/zh/changelog/` show published GitHub Releases in descending version order (`YYYYMMDD-N`, comparing the daily sequence numerically), preserving the original release-note language. Historical prereleases are labeled; drafts and records without a publication date are excluded.
- Each page has its own canonical URL, localized metadata and reciprocal `en`, `zh-CN`, and `x-default` alternates. The sitemap includes all six URLs. Language links preserve the current page; legacy `?lang=zh`, `?lang=zh-CN`, and `?lang=en` links redirect in the browser while preserving other parameters and channel hashes. Browser preferences do not override the URL's language.
- The original homepage title and formula heading remain. The subtitle is “A native Kanban and terminal workbench for AI coding agents” (localized on `/zh/`). The homepage retains `WebSite` and `SoftwareApplication` JSON-LD; FAQ-only structured data has been removed.

## Source structure

- `templates/home.html` — shared homepage body with `data-i18n` keys and language/resource URL placeholders.
- `locales.json` — homepage translations and metadata; `pages.json` — installation, FAQ and document-page translations.
- `schema.json` — shared software/site metadata, localized by the generator.
- `styles.css` — original Monokai CRT design; `pages.css` — document layout and responsive corrections.
- `app.js` — demo scenes, desktop channel navigation, installation-mode selection and copy helpers. Copy labels are embedded in each generated page; content language is never rewritten at runtime.
- `releases.json` — checked-in published-release snapshot for deterministic offline builds. Refresh it intentionally when updating the snapshot; deployments fetch current releases independently.
- `../scripts/website.py` — generator, release fetching/filtering and conservative escaped Markdown rendering (headings, lists, paragraphs, code, emphasis and HTTP(S) links).
- `install.sh` — source compatibility symlink. The generator copies the canonical `../scripts/install.sh` into the output.
- `CNAME`, icons, `og-image.png`, `site.webmanifest`, `robots.txt`, `llms.txt` — public assets. Templates and content JSON are not deployed.

## Build and preview

```sh
mise run build-website
mise run test-website
python3 -m http.server 8080 --bind 127.0.0.1 --directory .local/website
```

Open `http://127.0.0.1:8080/`, `/zh/`, `/install/`, or `/changelog/`. Preview the generated directory, not the source directory. No Node package installation or frontend framework is required to build the site.

Fetch current release history during a build with authenticated GitHub CLI access:

```sh
mise run build-website -- --refresh-releases
```

This paginates the repository's GitHub Releases API, filters unpublished records and exposes only public release fields. An API failure fails the build rather than publishing empty or stale release history. The source snapshot is not rewritten by a build.

## Deployment and verification

The Pages workflow validates the generator, refreshes release history, and uploads only `.local/website`. It runs on relevant `main` pushes, manual dispatch, published-release changes, and successful completion of the `Release` workflow. The completion trigger also covers releases created using `GITHUB_TOKEN`, whose release event does not start another workflow. Release-triggered builds use the default branch's website source.

`mise run test-website` checks complete localized HTML and metadata, visible FAQ content, local links/assets/fragments, all sitemap language pairs, canonical installer equality, release coverage, draft exclusion and escaping of untrusted release notes. Browser acceptance must additionally cover both languages at 320/390/720/768/1280px, desktop channel navigation, mobile scrolling, installation tabs, actual clipboard copying, language links, legacy query redirects and JavaScript-disabled rendering. Rust/native app tests do not exercise this standalone website.

## License

Apache-2.0 · Copyright 2026 JJy <jjyruby@gmail.com>.
