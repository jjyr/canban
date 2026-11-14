<p align="center">
  <img src="crates/ui/assets/app-icon.svg" width="84" alt="Canban icon" />
</p>

<h1 align="center">Canban</h1>

<p align="center">
  <strong>Focus on Human Context.</strong><br />
  Organize human intent, reviews, and multi-agent workflows on Kanban — while driving native Agent CLIs directly through Herdr.
</p>

<p align="center">
  <strong>English</strong> · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="#philosophy">Philosophy</a> ·
  <a href="#highlights">Highlights</a> ·
  <a href="#installation">Installation</a> ·
  <a href="#related-software-and-acknowledgements">Related Software</a> ·
  <a href="#license">License</a>
</p>

<p align="center">
  <img src="docs/assets/readme-board.png" width="860" alt="Canban Board in Monokai theme" />
</p>

---

## Philosophy

1. **Kanban organizes human context** — specs, `#Card` references, screenshots, `@Agent` discussions, structured reports, and review gates stay together on each Card.
2. **Herdr drives native Agent CLIs with minimal interference** — run Claude Code, Codex, Antigravity (`agy`), Cursor CLI, or custom CLIs directly inside persistent PTYs with native TUIs, permissions, MCPs, skills, and session resume intact.
3. **Most work stays on Kanban and dispatches via workflows** — shape scope and review diffs on the Board while workflows isolate Git worktrees, dispatch agents, and queue merges automatically.

---

## Highlights

### Kanban for Human–Agent Collaboration

Move Cards across customizable stages (`Backlog → Develop → Review`) with live `@developer`, `@reviewer`, and `@merger` status capsules right on the Board. Each Card combines Markdown specs, pasted screenshots, `#Card` / `#tag` links, `Cmd+O` Quick Open, and GitHub Issue/PR sync with approval gates for external PRs.

### GitHub Actions-Style Declarative Workflows

Automate column transitions, `@agent` runs, custom scripts, conditions, human approvals, and serialized FIFO **Queue Merge** (`deliver` with automatic `@merger` conflict resolution) inside isolated per-Card Git worktrees — authored visually in Workflow Studio, in YAML, or via **Edit with agent**.

<p align="center">
  <img src="docs/assets/readme-workflow.png" width="840" alt="Canban Visual Workflow Studio in Monokai theme" />
</p>

### Native Herdr Terminal — Switch Anytime

Press `Cmd+J` on any Card for a side-by-side terminal split or open the dedicated **Terminals** workspace. Built in Rust with `longbridge/nsh` (AppKit/Metal) and `libghostty-vt` over local or remote SSH `herdr-core`, running agent PTYs survive app restarts and upgrades across 17 built-in dark themes.

<p align="center">
  <img src="docs/assets/readme-card-terminal.png" width="840" alt="Canban Card Detail and Herdr Terminal split view in Monokai theme" />
</p>

---

## Installation

**Prerequisites**: macOS 13.0+ (Apple Silicon), `git`, at least one native Agent CLI (`claude`, `codex`, `agy`, `cursor-agent`, etc.), and **Herdr Core (`hc`)** (Canban guides one-click `hc` setup on first launch if missing).

```bash
curl -fsSL https://raw.githubusercontent.com/jjyr/canban/main/scripts/install.sh | bash
```

Installs or live-upgrades `/Applications/Canban.app` without interrupting active `herdr-core` sessions, and automatically handles macOS Gatekeeper quarantine removal and local ad-hoc code signing.

---

## Related Software and Acknowledgements

Canban builds directly on these open-source projects and assets (full attributions and licenses are preserved in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)):

- **[Herdr (`herdr-core`)](https://github.com/herdrdev/herdr-core)** — Persistent PTY session supervisor and JSON-RPC runtime powering native Agent CLIs and interactive terminals (Apache-2.0)
- **[Sheprd](https://github.com/sheprd-labs/sheprd)** — Terminal VT/frame/input and subscription utility patterns referenced during integration (Apache-2.0)
- **[NSH (`longbridge/nsh`)](https://github.com/longbridge/nsh)** — Native macOS AppKit/Metal Rust UI framework (Apache-2.0)
- **[Ghostty (`libghostty-vt`)](https://github.com/ghostty-org/ghostty)** — Commit-pinned VT parser and terminal emulation engine (MIT)
- **Native Agent CLIs** — Compatible with [Claude Code](https://docs.anthropic.com/en/docs/claude-code), [OpenAI Codex CLI](https://github.com/openai/codex), Antigravity (`agy`), Cursor CLI, and custom CLI profiles
- **[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)** (MIT) — 16 classic terminal color palettes including Monokai, Dracula, Nord, Tokyo Night, and Catppuccin Mocha
- **[Lucide](https://lucide.dev)** (ISC), **[Lobe Icons](https://github.com/lobehub/lobe-icons)** (MIT), and **[Primer Octicons](https://github.com/primer/octicons)** (MIT) — UI and provider icons

---

## License

Copyright 2026 JJy \<jjyruby@gmail.com\>.

Licensed under the [Apache License, Version 2.0](LICENSE).
