<h1 align="center">
  <a href="https://canban.dev"><img src="crates/ui/assets/app-icon.svg" alt="Canban" width="56" valign="middle" /></a> Canban
</h1>

<p align="center">
  <strong>Organize with Kanban. Work with native agent CLIs.</strong><br/>
  A lightweight, native <strong>Rust + <code>libghostty</code></strong> app for <strong>Claude Code, Codex, Cursor CLI, Antigravity (<code>agy</code>), and more</strong> — zero Electron, zero chat wrapper, with a noise-filtered Daily Briefing Inbox, restart-safe PTYs, and per-card Git worktree review.
</p>

<p align="center">
  <a href="https://github.com/jjyr/canban/releases"><img src="https://img.shields.io/github/v/release/jjyr/canban?label=release" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/jjyr/canban" alt="Apache-2.0 License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Android-blue" alt="macOS, Linux, and Android" />
  <img src="https://img.shields.io/badge/built%20with-Rust-orange" alt="Built with Rust" />
</p>

<p align="center">
  <a href="#installation"><strong>Quick Install</strong></a> · <a href="https://canban.dev"><strong>canban.dev</strong></a> · <a href="https://canban.dev/docs/"><strong>Docs</strong></a>
  <br/>
  <sub><strong>English</strong> · <a href="README.zh-CN.md">简体中文</a></sub>
</p>

---

## Why Canban?

- **Start from one Daily Briefing**: One noise-filtered Inbox across all projects, GitHub PRs, scheduled runs, and remote hosts — unblock waiting agents inline, merge green PRs in one click, and keep passive noise folded out of sight.
- **Organize with Kanban**: One board per local directory or Git repo (`Backlog → Develop → Review`). Keep task specs, comments, worktrees, and inline diffs together on one card with zero setup ceremony.
- **Work with native agent CLIs**: Run your real `claude`, `codex`, `agy`, `cursor-agent`, or `pi` CLI inside restart-safe persistent PTYs. Full native TUI, permissions, MCPs, skills, and session resume.
- **Lightweight & native**: Built in Rust + NSH + `libghostty-vt`. No Electron, no Chromium, no cloud account — just a fast local app backed by SQLite that scales directly to remote Linux `canband` hosts and Android.

## Features

### Daily Briefing Inbox

Start every session from one calm, noise-filtered briefing across all projects, worktrees, GitHub PRs, scheduled runs, and remote hosts. Triage with clickable decision filters (`Waiting On You`, `Ready to Merge`, `Agents`, `Shipped Today`), reply to blocked agents or PRs inline, read PR descriptions and CI checks without leaving the app, and jump straight into any Card or live terminal.

<p align="center">
  <img src="docs/assets/daily-briefing.webp" alt="Canban Daily Briefing Inbox triaging waiting agents, active sessions across local and remote hosts, and GitHub PRs" width="100%" />
</p>

### Kanban Board &amp; Card Context

Organize work around any local directory or Git repo across local and remote hosts with a fast `Backlog → Develop → Review` board. Keep Markdown specs, screenshots, `#Card` and GitHub Issue/PR links, and `@Agent` discussions together in one place.

<p align="center">
  <img src="docs/assets/board.webp" alt="Canban Kanban board organizing Backlog, Develop, and Review columns across local and remote projects" width="100%" />
</p>

### Native Agent CLIs &amp; Persistent Terminals

No dumbed-down chat wrapper. Run Claude Code, Codex, Antigravity (`agy`), Cursor CLI, Pi, or custom CLIs inside persistent, restart-safe PTYs powered by Rust, NSH, and `libghostty-vt` across local and remote machines — preserving native TUIs, interactive permissions, MCPs, skills, and session resume. [Docs →](https://canban.dev/docs/agents/)

<p align="center">
  <img src="docs/assets/terminals.webp" alt="Canban Terminals workspace running native Agent CLIs across local macOS and remote Linux hosts" width="100%" />
</p>

### Per-Card Worktrees &amp; Inline Diff Review

Inspect commits and isolated per-card worktree diffs without switching to an external editor. Leave inline review comments directly on changed lines and send focused revision requests straight back to `@Agent`. [Docs →](https://canban.dev/docs/)

<p align="center">
  <img src="docs/assets/git-review.webp" alt="Canban built-in Git workbench showing commit history, changed files, and inline diff review" width="100%" />
</p>

### Scheduled Runs &amp; Workflows

Run recurring agent tasks on a cron schedule and inspect results or retry failures straight from your Inbox. Build GitHub Actions-like local YAML workflows visually or in source mode — combining shell scripts, `agent` + prompt steps, and a serialized merge queue. [Docs →](https://canban.dev/docs/workflows/)

<p align="center">
  <img src="docs/assets/workflows.webp" alt="Canban Workflows Visual Studio showing Backlog, Develop, and Review automation stages and merge queue" width="100%" />
</p>

### Multi-Machine &amp; Mobile (Direct E2E)

Offload heavy builds and agent runs to remote Linux boxes. Connect your lightweight desktop or mobile app directly to headless `canband` hosts with mandatory end-to-end encryption and zero relay servers — optionally paired with **Tailscale** or **SSH** — to control remote boards, Inbox triage, workflows, and live 120Hz terminals seamlessly. [Docs →](https://canban.dev/docs/remote-hosts/)

## Privacy &amp; Security

- **Zero data collection**: No cloud account, no telemetry, no analytics, and no tracking. Your boards, cards, workflows, and credentials stay on your own machines in local SQLite (with secrets encrypted at rest).
- **Direct end-to-end encrypted (no relay servers)**: Desktop, mobile, and remote `canband` hosts connect directly to each other with mandatory end-to-end encryption and public-key verification — traffic never passes through any third-party relay or Canban server.
- **Optional Tailscale or SSH**: Works over direct LAN/VPN connections out of the box, or optionally pair with **Tailscale** (for private mesh networking and same-account auto-auth) or **SSH**.

For cryptographic details (Noise protocol, host key pinning, and secret vault encryption), see [Privacy &amp; Security Docs →](https://canban.dev/docs/#privacy-security).

## Installation

Install on **macOS 13+** (Apple Silicon) or **Linux Desktop** (`x86_64` / `arm64`) with a single command:

```bash
curl -fsSL https://canban.dev/install.sh | bash
```

Works out of the box with `git` and your existing Agent CLIs (`claude`, `codex`, `agy`, `cursor-agent`, etc.). On first launch, Canban automatically guides one-click setup for the persistent PTY daemon (`hc`) if not already installed.

### Android Companion (`arm64-v8a`)

<p>
  <a href="https://github.com/jjyr/canban/releases/latest"><img src="https://img.shields.io/badge/Android-Download%20APK-3DDC84?style=for-the-badge&logo=android&logoColor=white" alt="Download Android APK" /></a>
  <a href="https://apps.obtainium.imranr.dev/redirect?r=obtainium://app/%7B%22id%22%3A%22dev.canban.mobile%22%2C%22url%22%3A%22https%3A%2F%2Fgithub.com%2Fjjyr%2Fcanban%22%2C%22author%22%3A%22jjyr%22%2C%22name%22%3A%22Canban%22%7D"><img src="https://img.shields.io/badge/Get_it_on-Obtainium-3A7AFE?style=for-the-badge&logo=android&logoColor=white" alt="Get it on Obtainium" /></a>
  <a href="https://github.com/jjyr/canban/issues/2"><img src="https://img.shields.io/badge/Need_iOS%3F-GitHub_Issue_%232-111111?style=for-the-badge&logo=apple&logoColor=white" alt="Need iOS?" /></a>
</p>

Connects directly to your macOS or headless Linux `canband` host with end-to-end encryption and zero relay servers — using single-use `canban://` QR pairing or optionally **Tailscale** (zero-QR WhoIs auto-auth) — for live Daily Briefing triage with native swipe gestures, Kanban boards, Git diffs, and persistent `libghostty-vt` terminals.

<p align="center">
  <img src="docs/assets/mobile-inbox.webp" alt="Canban mobile Daily Briefing Inbox with swipe triage and active agents" width="31%" />
  &nbsp;
  <img src="docs/assets/mobile-board.webp" alt="Canban mobile Kanban board showing active cards and worktree branches" width="31%" />
  &nbsp;
  <img src="docs/assets/mobile-terminal.webp" alt="Canban mobile live libghostty-vt terminal running Claude Code" width="31%" />
</p>

<details>
<summary><strong>Linux runtime dependencies &amp; building from source</strong></summary>

<br/>

**Linux Desktop Runtime Libraries**:
- GTK4 and GStreamer (`libgtk-4-1` / `gtk4` + `gstreamer`)

**Build from Source** (requires `mise` and platform build dependencies):

- **macOS**: `brew install librsvg`
- **Ubuntu / Debian**: `sudo apt install pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev librsvg2-bin zstd`
- **Arch Linux**: `sudo pacman -S pkgconf gtk4 gstreamer gst-plugins-base librsvg zstd`
- **Fedora**: `sudo dnf install pkgconf-pkg-config gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel librsvg2-tools zstd`

```bash
git clone https://github.com/jjyr/canban.git && cd canban && mise run install-app
```

</details>

## Related Software and Acknowledgements

Canban builds on these open-source projects (full attributions in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)):

- **[Herdr](https://github.com/herdrdev/herdr-core)** - Persistent PTY session supervisor for native Agent CLIs
- **[Sheprd](https://github.com/sheprd-labs/sheprd)** - Terminal VT and subscription patterns
- **[NSH](https://github.com/longbridge/nsh)** - Native macOS and Linux Rust UI framework
- **[Ghostty](https://github.com/ghostty-org/ghostty)** - `libghostty-vt` terminal emulation engine
- **[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)** - Built-in terminal color palettes
- **[Lucide](https://lucide.dev)**, **[Lobe Icons](https://github.com/lobehub/lobe-icons)**, **[Primer Octicons](https://github.com/primer/octicons)** - UI and provider icons

## License

Copyright 2026 JJy \<jjyruby@gmail.com\>.

Licensed under the [Apache License, Version 2.0](LICENSE).
