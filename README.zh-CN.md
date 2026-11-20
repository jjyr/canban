<h1 align="center">
  <a href="https://canban.dev/zh/"><img src="crates/ui/assets/app-icon.svg" alt="Canban" width="56" valign="middle" /></a> Canban
</h1>

<p align="center">
  <a href="https://github.com/jjyr/canban/releases"><img src="https://img.shields.io/github/v/release/jjyr/canban?label=release" alt="最新版本" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/jjyr/canban" alt="Apache-2.0 协议" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-blue" alt="支持 macOS 与 Linux" />
  <img src="https://img.shields.io/badge/built%20with-Rust-orange" alt="使用 Rust 构建" />
</p>

<p align="center">
  <sub><a href="README.md">English</a> · <strong>简体中文</strong></sub>
</p>

<p align="center">
  <strong>保留你的原生 Agent CLI，自动化你的工作流。</strong><br/>
  用灵活的看板沉淀任务上下文，用通知中心掌握等待中的 Agent，用类似 GitHub Actions 的工作流自动化执行任务。
</p>

<h3 align="center"><a href="#安装"><ins>安装 Canban</ins></a> · <a href="https://canban.dev/zh/"><ins>canban.dev</ins></a></h3>

<p align="center">
  <img src="docs/assets/canban-hero.gif" alt="Canban 看板、展开卡片与实时原生 Agent CLI 终端演示" width="960" />
</p>

## 核心功能

<table>
<tr>
<td width="50%" valign="middle">

### 可编程工作流（Vibe Mode）

用类似 GitHub Actions 的 YAML 编排 Shell 脚本与 `agent` + prompt 步骤。将任意卡片转化为 `plan → develop → merge` 自动化流水线，内置独立 Git Worktree、定时任务（Cron）与串行合并队列。

[文档 →](https://canban.dev/zh/docs/workflows/)

</td>
<td width="50%">
  <img src="website/screenshots/workflow-studio.png" alt="Canban 可编程工作流：卡片从 plan 到 develop 再到 merge 自动推进" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### 原生 Agent CLI + 瑞士军刀

在持久化、重启不中断的 PTY 中直接运行 Claude Code、Codex、Antigravity（`agy`）、Cursor CLI、Pi 或自定义 CLI 同时配齐通知中心（`⌘I`）、内置 Git Diff 审查，以及 Rust 原生应用的轻量与流畅。

[文档 →](https://canban.dev/zh/docs/agents/)

</td>
<td width="50%">
  <img src="website/screenshots/review-diff.png" alt="Canban 原生终端、通知中心与内置 Git Diff 审查" width="100%" />
</td>
</tr>
<tr>
<td width="50%" valign="middle">

### 多机器协作

通过 SSH 或 Tailscale 将本地桌面端连接到无头 Linux `canband` 主机，与本地项目一起无缝管理远程看板、工作流和实时终端。

[文档 →](https://canban.dev/zh/docs/remote-hosts/)

</td>
<td width="50%">
  <img src="docs/assets/multi-machine.png" alt="Canban Connected Hosts 弹窗：展示本地 macOS Socket 与 Tailscale TCP 远程 Linux 主机" width="100%" />
</td>
</tr>
</table>

## 安装

**前置要求**：
- **macOS**：macOS 13.0+（Apple Silicon / `arm64`）
- **Linux 桌面版**：`x86_64` 或 `aarch64`（`arm64`），已安装 GTK4 与 GStreamer 运行库（`libgtk-4-1` / `gtk4` + `gstreamer`）
- **通用**：`git`、至少一个原生 Agent CLI（如 `claude`、`codex`、`agy`、`cursor-agent` 等），以及 **Herdr Core（`hc`）**（首次启动时若缺失会引导一键安装）。

```bash
curl -fsSL https://canban.dev/install.sh | bash
```

### 从源码构建

安装 `mise` 与系统构建依赖后执行 `mise run install-app`：

- **macOS**：`brew install librsvg`
- **Ubuntu / Debian**：`sudo apt install pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev librsvg2-bin zstd`
- **Arch Linux**：`sudo pacman -S pkgconf gtk4 gstreamer gst-plugins-base librsvg zstd`
- **Fedora**：`sudo dnf install pkgconf-pkg-config gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel librsvg2-tools zstd`

```bash
git clone https://github.com/jjyr/canban.git && cd canban && mise run install-app
```

## 相关软件与致谢

Canban 基于以下开源项目构建（完整归属与许可证见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)）：

- **[Herdr](https://github.com/herdrdev/herdr-core)** - 面向原生 Agent CLI 的持久化 PTY 会话监督引擎
- **[Sheprd](https://github.com/sheprd-labs/sheprd)** - 终端 VT 与订阅交互实践参考
- **[NSH](https://github.com/longbridge/nsh)** - 原生 macOS 与 Linux Rust UI 框架
- **[Ghostty](https://github.com/ghostty-org/ghostty)** - `libghostty-vt` 终端解析与渲染内核
- **[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)** - 内置终端配色方案
- **[Lucide](https://lucide.dev)**、**[Lobe Icons](https://github.com/lobehub/lobe-icons)**、**[Primer Octicons](https://github.com/primer/octicons)** - UI 与模型/服务商图标

## 开源协议

Copyright 2026 JJy \<jjyruby@gmail.com\>.

本项目基于 [Apache License, Version 2.0](LICENSE) 开源。
