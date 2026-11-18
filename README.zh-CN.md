<p align="center">
  <img src="crates/ui/assets/app-icon.svg" width="84" alt="Canban 图标" />
</p>

<h1 align="center">Canban</h1>

<p align="center">
  <strong>Focus on Human Context.</strong><br />
  Canban = Kanban + Workflow + Herdr
</p>

<p align="center">
  <img src="docs/assets/canban_intro.gif" width="1000" alt="Canban 演示：看板、工作流与 Herdr 终端" />
</p>

<p align="center">
  <a href="README.md">English</a> · <strong>简体中文</strong>
</p>

<p align="center">
  <a href="https://canban.dev/?lang=zh"><strong>官网 (canban.dev)</strong></a> ·
  <a href="#设计哲学">设计哲学</a> ·
  <a href="#核心亮点">核心亮点</a> ·
  <a href="#安装">安装</a> ·
  <a href="#相关软件与致谢">相关软件与致谢</a> ·
  <a href="#开源协议">开源协议</a>
</p>

---

## 设计哲学

1. **用 Kanban 组织人类上下文** — 需求说明、`#Card` 关联、截图、`@Agent` 讨论、结构化报告与审查门禁统一沉淀在每张 Card 上。
2. **通过 Herdr 调用原生 Agent CLI，尽量减少干涉** — 在持久化 PTY 中直接运行 Claude Code、Codex、Antigravity (`agy`)、Cursor CLI 等原生工具，完整保留原生 TUI、权限交互、MCP、Skills 与会话恢复。
3. **大部分工作在 Kanban 完成，并通过 Workflow 派发执行** — 人在看板上把控需求范围、答疑与代码审查，Worktree 隔离、Agent 调度与队列合并交由工作流自动完成。

---

## 核心亮点

- **用看板保留任务上下文** — 每张卡片集中记录功能需求、截图、与 Agent 的讨论和评审决定。在看板上就能实时查看 Agent 的进度。

- **自动化编码工作流** — 将卡片拖入 `Develop` 列，即可自动派发编码 Agent。像使用 GitHub Actions 一样，用自定义脚本、条件判断和人工审批步骤编排工作流。

- **独立的 Git 工作树与合并队列** — 每张卡片在独立的 Git 工作树中执行任务。变更通过本地合并队列依次合入，遇到冲突时由 Agent 协助解决。

- **原生 Agent CLI** — 通过 Herdr 运行 Claude Code、Codex、Antigravity、Cursor CLI 和自定义 Agent，保留各自的终端界面、权限机制、MCP 和技能。

- **重启无中断执行** — 即使 Canban 重启或升级，编码 Agent 也能继续运行，不中断正在进行的工作。

- **远程机器支持** — 通过 SSH/Tailscale 连接远程 Linux 机器，并在远程执行编码 Agent。

- **GitHub 集成** — 将 GitHub Issues 和 PR 转化为看板卡片，像本地卡片一样在其上运行工作流。

---

## 安装

**前置要求**：
- **macOS**：macOS 13.0+（Apple Silicon / `arm64`）
- **Linux 桌面版**：`x86_64` 或 `aarch64` (`arm64`)，已安装 GTK4 与 GStreamer 运行库（`libgtk-4-1` / `gtk4` + `gstreamer`）
- **通用**：`git`、至少一个原生 Agent CLI（如 `claude`、`codex`、`agy`、`cursor-agent`），以及 **Herdr Core (`hc`)**（首次启动时若缺失会引导一键安装）。

```bash
curl -fsSL https://canban.dev/install.sh | bash
```

- **macOS**：安装或热升级 `/Applications/Canban.app`，自动清理 Gatekeeper 隔离属性并完成本地签名处理。
- **Linux 桌面版**：安装或热升级 `~/.local/opt/canban`，自动创建 `~/.local/bin/{canban,canban-ui}` 符号链接并注册 XDG `.desktop` 启动器与图标（`~/.local/share/applications` 与 `icons/hicolor`）。
- **Linux 无头服务端 (`canband`)**：在无图形环境的远程主机上可传入 `--headless`（`curl -fsSL https://canban.dev/install.sh | bash -s -- --headless`），仅安装独立的 `~/.local/bin/canban` 守护进程，无需 GTK4 依赖；升级时保留运行中的 `herdr-core` 会话并自动协调 `systemd --user` `canban.service`。

### 从源码构建

安装 `mise` 与系统构建依赖后执行 `mise run install-app`：

- **macOS**：`brew install librsvg`
- **Ubuntu / Debian**：`sudo apt install pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev librsvg2-bin zstd`
- **Arch Linux**：`sudo pacman -S pkgconf gtk4 gstreamer gst-plugins-base librsvg zstd`
- **Fedora**：`sudo dnf install pkgconf-pkg-config gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel librsvg2-tools zstd`

```bash
git clone https://github.com/jjyr/canban.git && cd canban && mise run install-app
```

---

## 相关软件与致谢

Canban 的实现离不开以下优秀开源项目与资源（完整的第三方版权与许可证文本保留在 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)）：

- **[Herdr (`herdr-core`)](https://github.com/herdrdev/herdr-core)** — 负责 PTY 会话托管、生命周期监督与 JSON-RPC 通信的底层引擎（Apache-2.0）
- **[Sheprd](https://github.com/sheprd-labs/sheprd)** — 在终端 VT/输入封装与订阅工具契约中参考了其工程实践（Apache-2.0）
- **[NSH (`longbridge/nsh`)](https://github.com/longbridge/nsh)** — 原生 macOS（AppKit / Metal）与 Linux（GTK4）Rust 桌面 UI 框架（Apache-2.0）
- **[Ghostty (`libghostty-vt`)](https://github.com/ghostty-org/ghostty)** — 按固定提交锁定的高性能 VT 终端解析内核（MIT）
- **[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)** (MIT) — 16 套经典暗色终端配色方案（含 Monokai、Dracula、Nord、Tokyo Night、Catppuccin Mocha 等）
- **[Lucide](https://lucide.dev)** (ISC)、**[Lobe Icons](https://github.com/lobehub/lobe-icons)** (MIT)、**[Primer Octicons](https://github.com/primer/octicons)** (MIT) — UI 图标与模型/服务商标识

---

## 开源协议

Copyright 2026 JJy \<jjyruby@gmail.com\>.

本项目基于 [Apache License, Version 2.0](LICENSE) 开源。
