<h1 align="center">
  <a href="https://canban.dev"><img src="crates/ui/assets/app-icon.svg" alt="Canban" width="56" valign="middle" /></a> Canban
</h1>

<p align="center">
  <a href="https://github.com/jjyr/canban/releases"><img src="https://img.shields.io/github/v/release/jjyr/canban?label=release" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/jjyr/canban" alt="Apache-2.0 License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Android-blue" alt="macOS, Linux, and Android" />
  <img src="https://img.shields.io/badge/built%20with-Rust-orange" alt="Built with Rust" />
</p>

<p align="center">
  <sub><strong>English</strong> · <a href="README.zh-CN.md">简体中文</a></sub>
</p>

<p align="center">
  <strong>Run 10+ coding agents without drowning in terminal tabs.</strong><br/>
  A native <strong>Rust + <code>libghostty</code></strong> workbench for <strong>Claude Code, Codex, and Cursor CLI</strong> - with an instant attention inbox (<code>⌘I</code>), per-card Git worktree review, and GitHub Actions-like local workflows.
</p>

<h3 align="center"><a href="#installation"><ins>Quick Install</ins></a> · <a href="https://canban.dev"><ins>canban.dev</ins></a> · <a href="https://canban.dev/docs/"><ins>Docs</ins></a></h3>

<p align="center">
  <img src="docs/assets/canban-hero.gif" alt="Canban Kanban board with an expanded card and live native Agent CLI terminal" width="960" />
</p>

## Why Canban?

- **Stop hunting through 10 tmux panes**: One global attention inbox (`⌘I`) tells you the exact moment an agent pauses for `[Y/n]` permission, input, or code review across every project and host.
- **Zero-wrapper native terminals**: Keep your real `claude`, `codex`, `agy`, or `cursor-agent` CLI. Powered by native Rust UI and `libghostty-vt` over persistent PTYs that survive app restarts.
- **Review and merge without leaving the app**: Every card gets an isolated Git worktree, built-in inline diff review (`@Agent` comments on specific lines), and an automated `plan → develop → merge` queue.

## Features

<table>
<tr>
<td width="50%" valign="middle">

### Attention Inbox (`⌘I`)

Never miss a blocked agent again. One global notification center across all projects, worktrees, and remote hosts shows which agent is waiting for input, tool approval, or review - and jumps straight to its Card and live terminal in one keystroke.

</td>
<td width="50%">
  <img src="docs/assets/notification-center.jpg" alt="Canban Notification Center showing waiting agents and one-click jump to Card and terminal" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### Native Agent CLI &amp; Persistent Terminals

No dumbed-down chat wrapper. Run Claude Code, Codex, Antigravity (`agy`), Cursor CLI, Pi, or custom CLIs inside persistent, restart-safe PTYs powered by Rust, NSH, and `libghostty-vt` - preserving native TUIs, interactive permissions, MCPs, skills, and session resume.

[Docs →](https://canban.dev/docs/agents/)

</td>
<td width="50%">
  <img src="docs/assets/terminals-cli.jpg" alt="Canban Terminals workspace running native Agent CLIs" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### Per-Card Worktrees &amp; Inline Diff Review

Inspect commits and isolated per-card worktree diffs without switching to an external editor. Leave inline review comments directly on changed lines and send focused revision requests straight back to `@Agent`.

[Docs →](https://canban.dev/docs/)

</td>
<td width="50%">
  <img src="website/screenshots/review-diff.png" alt="Canban built-in Git workbench and inline diff review" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### Programmable Workflows (GitHub Actions for Local Agents)

Ready to automate the loop? Write GitHub Actions-like YAML combining shell scripts and `agent` + prompt steps. Turn any card into a hands-free `plan → develop → merge` pipeline with cron schedules and a serialized merge queue.

[Docs →](https://canban.dev/docs/workflows/)

</td>
<td width="50%">
  <img src="website/screenshots/workflow-studio.png" alt="Canban Programmable Workflows: Visual Studio and Vibe Mode pipeline" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### Multi-Machine over SSH &amp; Tailscale

Offload heavy builds and agent runs to remote Linux boxes. Connect your local macOS/Linux desktop app to headless `canband` hosts over SSH or Tailscale and control remote boards, workflows, and live 120Hz terminals seamlessly.

[Docs →](https://canban.dev/docs/remote-hosts/)

</td>
<td width="50%">
  <img src="docs/assets/multi-machine.png" alt="Canban Connected Hosts popover showing local macOS socket and remote Linux host over Tailscale TCP" width="100%" />
</td>
</tr>
</table>

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
</p>

Connects to your macOS or headless Linux `canband` host over Tailscale (zero-QR WhoIs auto-auth) or single-use `canban://` QR pairing for live Needs-you triage, Kanban boards, Git diffs, and persistent `libghostty-vt` terminals.

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
