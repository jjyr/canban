# Canban Website (`canban.dev`)

Static product site for [https://canban.dev/](https://canban.dev/), deployed via GitHub Pages (`.github/workflows/pages.yml`).

## Structure

- `index.html` — 4-channel retro CRT monitor deck (`CH.01` Workbench, `CH.02` Human Context, `CH.03` Parallel Pipeline, `CH.04` Install).
- `styles.css` — Native Monokai dark CRT design system aligned with Canban's built-in Monokai palette (`crates/themes/src/lib.rs`), phosphor scanlines/noise, and pixel-aligned wireframe flow.
- `app.js` — Concise `en` / `zh` localization, discrete channel paging with CRT H-sync glitch transitions, 3-primitive scene sequencer (`Kanban`, `Workflow`, `Herdr`), and clipboard helpers.
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
