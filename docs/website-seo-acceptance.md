# Website localization and SEO acceptance

Date: 2026-11-18 (Asia/Shanghai). Source baseline: `fa14d95`; this report records local verification of the website changes prepared on that baseline. This report covers the standalone public website, not the native desktop app.

## Implemented behavior

- The standard-library generator builds six complete HTML pages: `/`, `/zh/`, `/install/`, `/zh/install/`, `/changelog/`, and `/zh/changelog/`.
- Homepage title and formula remain; the approved native Kanban/terminal workbench subtitle replaces the original subtitle. Desktop retains four channels. Phones use ordinary vertical scrolling, with all four sections available.
- Language is determined by the URL. Each page has a self-referencing canonical, localized title/description/OG metadata, reciprocal language alternates and sitemap inclusion. Language links preserve the page. Legacy language query parameters redirect in JavaScript, preserving other query parameters and fragments.
- Installation pages contain platform requirements, desktop/headless commands, runtime dependencies, updates, source links and six visible FAQs. The Agent answer explicitly follows Herdr support, with representative agent examples. The homepage's invisible FAQ-only JSON-LD is removed.
- Release history is rendered into HTML using published GitHub Releases. Drafts and unpublished records are excluded; original notes and historical prerelease labels are retained. `YYYYMMDD-N` versions are compared numerically. Release-note HTML and unsafe link schemes cannot become active page markup.
- Pages builds refresh release history. Release changes and successful completion of the manual `Release` workflow also trigger deployment, including releases produced using `GITHUB_TOKEN`.

See [website source and build instructions](../website/README.md).

## Verification

The generated site was served at `http://127.0.0.1:8765/` and exercised in an isolated Playwright/Chrome session.

- `mise run test-website`: six focused tests passed. Coverage includes localized HTML/metadata, visible FAQ answers, canonical/alternate URLs, all internal links/assets/fragments, sitemap reciprocity, installer equality, full release coverage, draft filtering, safe Markdown rendering, and numeric release ordering with delayed publication.
- `mise run build-website -- --refresh-releases`: passed with actual paginated GitHub release metadata. The observed history contained 44 published releases, including `20261118-1`.
- `node --check website/app.js`, `git diff --check`, and `mise exec actionlint@1.7.12 shellcheck@0.11.0 -- actionlint .github/workflows/pages.yml`: passed. The lint tools were selected temporarily without changing pinned tool versions.
- All six routes were checked at widths 320, 390, 720, 768 and 1280px (30 combinations, height 844px). Document scroll width matched viewport width and the main heading stayed within the viewport in every combination. No page JavaScript errors were observed.
- Actual mouse-wheel input at 390px scrolled every route by 800px. A root-level `overflow: hidden` inheritance defect was discovered during visual QA, fixed on the document/mobile roots, and the complete viewport matrix was rerun.
- Desktop channel links selected each of the four corresponding sections while retaining `/zh/`. Desktop/headless tab selection and actual clipboard contents were checked. The Install resource link, page-preserving English switch and Install-page copy button were exercised.
- The Linux dependency disclosure was opened, and the Changelog navigation and Chinese language switch were exercised.
- Language switching retained `#ch-3` and restored the desktop Install channel; switching `/zh/install/#faq` to English retained `/install/#faq`.
- Legacy links were exercised: `/install/?lang=zh&utm_source=acceptance#faq` became `/zh/install/?utm_source=acceptance#faq`; `/zh/changelog/?lang=en#20261118-1` became `/changelog/#20261118-1`; `/?lang=zh-CN#ch-3` became `/zh/#ch-3`.
- With JavaScript disabled, all six routes returned their own language, heading and canonical; installation pages exposed all six FAQs and changelog pages exposed all 44 releases. On desktop, all four homepage sections were visible and actual wheel input scrolled the document by 900px.
- Homepage, mobile installation/FAQ and changelog screenshots were visually inspected. Long installation commands scroll within their own containers while copy controls remain visible.

## Verification boundary

This is local website acceptance. At the time of this verification, the modified GitHub Pages workflow had not run remotely and the production domain still served the previous deployment. Search Console indexing, selected canonicals, ranking changes and Core Web Vitals were not measured. No Rust/native UI code, prompts, workflow definitions or desktop API contracts were changed; native app build/gallery/acceptance suites were not run for this website-only change.

## Expanded content matrix and GEO update (2026-11-19)

- Updated `CH.01`, `CH.04` `.page-links`, and doc header navigation on `/` and `/zh/` to link to `/install/`, `/docs/`, `/docs/workflows/`, `/docs/cli/`, and `/changelog/` (with localized paths for `/zh/`), removing `/compare/` and `/workflows/` completely.
- Configured `scripts/website.py` and `website/docs.json` to generate 16 bilingual HTML pages (`/` + `install` + `docs` + `docs/workflows` + `docs/cli` + `docs/agents` + `docs/remote-hosts` + `changelog` in `en` and `zh`), each with self-referencing canonicals, reciprocal `hreflang` links, `<link rel="alternate" type="text/markdown">` mirrors, `BreadcrumbList`, and `TechArticle` JSON-LD on documentation pages.
- Refocused `/docs/workflows/` (and `/zh/docs/workflows/`) on `schema: 10` Workflow YAML syntax and execution semantics verified directly against `crates/workflow-engine/src/{definition.rs,stages.rs,source.rs,migrations.rs,runtime.rs}`, and added `/docs/cli/` (and `/zh/docs/cli/`) covering core CLI commands, Agent-first JSON/response shaping (`--fields`, `--full`, `request_key`, `CANBAN_*` injection, `canban skill`, `review draft`), and human usage (`--human`).
- Added 16 clean `.md` mirror endpoints alongside `llms-full.txt` and updated `website/llms.txt` with `Canban = Kanban + Workflow + Herdr` disambiguation.
- Verified with `mise run build-website` and `mise run test-website` (including route exclusion assertions, `TechArticle`/`BreadcrumbList` JSON-LD, `.md` mirrors, `llms-full.txt`, `screenshots/*.png`, and 24-CLI parity against `crates/profiles/src/catalog.rs`). Note: the Playwright multi-viewport browser matrix above was recorded on the earlier 6-route baseline and was not re-run for the updated doc routes.
