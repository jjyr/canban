# Canban Website (`canban.dev`)

Static product site for [https://canban.dev/](https://canban.dev/), deployed via GitHub Pages (`.github/workflows/pages.yml`).

## Structure

- `index.html` — Zen product showcase (default English HTML, pre-paint browser locale matching, interactive parallel Kanban + Herdr Workbench preview, copyable install command, and GitHub Star prompt).
- `styles.css` — Responsive warm-paper / dark-ink design system and subtle motion choreography (`prefers-reduced-motion` aware).
- `app.js` — Client-side i18n dictionary (`en` / `zh`, extensible), interactive Card context switching, ambient stage preview, and clipboard actions.
- `install.sh` — Shell installer served at `https://canban.dev/install.sh`.
- `CNAME` — Custom domain binding (`canban.dev`).
- `favicon.svg` — Vector Canban mark derived from `crates/ui/assets/app-icon.svg`.

## Local Preview

No build step or bundler is required:

```sh
python3 -m http.server 8080 --directory website
```

Then open `http://127.0.0.1:8080` in a browser.

## License

Apache-2.0 · Copyright 2026 JJy <jjyruby@gmail.com>.
