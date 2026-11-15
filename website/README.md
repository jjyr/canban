# Canban Website (`canban.dev`)

Static product site for [https://canban.dev/](https://canban.dev/), deployed via GitHub Pages (`.github/workflows/pages.yml`).

## Structure

- `index.html` — 4-channel retro CRT monitor deck (`CH.01` Workbench, `CH.02` Human Context, `CH.03` Parallel Pipeline, `CH.04` Install) with Open Graph / Twitter Cards, `hreflang`, and `Schema.org` JSON-LD (`WebSite`, `SoftwareApplication`, `FAQPage`).
- `styles.css` — Native Monokai dark CRT design system aligned with Canban's built-in Monokai palette (`crates/themes/src/lib.rs`), phosphor scanlines/noise, and pixel-aligned wireframe flow.
- `app.js` — Concise `en` / `zh` localization (`?lang=zh` URL state), discrete channel paging (`#ch-0`..`#ch-3`) with CRT H-sync glitch transitions, 3-primitive scene sequencer (`Kanban`, `Workflow`, `Herdr`), and clipboard helpers.
- `install.sh` — Symlink to the canonical [`../scripts/install.sh`](../scripts/install.sh), materialized during Pages deploy and served at `https://canban.dev/install.sh`.
- `CNAME` — Custom domain binding (`canban.dev`).
- `favicon.svg`, `favicon-48x48.png`, `icon-192.png`, `icon-512.png`, `apple-touch-icon.png`, `site.webmanifest` — Vector and raster app/SERP icons.
- `og-image.png` — 1200×630 Open Graph / Twitter social preview card.
- `robots.txt`, `sitemap.xml`, `llms.txt` — Search crawler directives, bilingual XML sitemap, and LLM summary index.

## Local Preview

No build step or bundler is required:

```sh
python3 -m http.server 8080 --directory website
```

Then open `http://127.0.0.1:8080` in a browser.

## License

Apache-2.0 · Copyright 2026 JJy <jjyruby@gmail.com>.
