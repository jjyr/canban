<p align="center">
  <img src="crates/ui/assets/app-icon.svg" width="84" alt="Canban 图标" />
</p>

<h1 align="center">Canban</h1>

<p align="center">
  <strong>Focus on Human Context.</strong><br />
  Canban = Kanban + Workflow + Herdr
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

<p align="center">
  <img src="docs/assets/readme-board.png" width="860" alt="Monokai 主题下的 Canban 看板视图" />
</p>

---

## 设计哲学

1. **用 Kanban 组织人类上下文** — 需求说明、`#Card` 关联、截图、`@Agent` 讨论、结构化报告与审查门禁统一沉淀在每张 Card 上。
2. **通过 Herdr 调用原生 Agent CLI，尽量减少干涉** — 在持久化 PTY 中直接运行 Claude Code、Codex、Antigravity (`agy`)、Cursor CLI 等原生工具，完整保留原生 TUI、权限交互、MCP、Skills 与会话恢复。
3. **大部分工作在 Kanban 完成，并通过 Workflow 派发执行** — 人在看板上把控需求范围、答疑与代码审查，Worktree 隔离、Agent 调度与队列合并交由工作流自动完成。

---

## 核心亮点

### 面向人机协作的 Kanban 看板

Card 沿 `Backlog → Develop → Review`（或自定义列）流转，卡片上实时展示 `@developer`、`@reviewer`、`@merger` 运行状态胶囊；内置 Markdown 编辑预览、剪贴板粘贴图片、`#Card` / `#tag` 引用补全、`Cmd+O` 快速跳转，以及 GitHub Issues / PR 同步与外部 PR 审批门禁。

### 类 GitHub Actions 的声明式 Workflow

为每张 Card 自动隔离独立 Git Worktree，串联列流转、`@agent` 执行、自定义脚本、条件分支、人工审批与 FIFO **Queue Merge**（冲突时自动交由 `@merger` 处理）；支持在可视化 Workflow Studio、YAML 源码与 **Edit with agent** 之间随时切换。

<p align="center">
  <img src="docs/assets/readme-workflow.png" width="840" alt="Monokai 主题下的 Canban 可视化 Workflow Studio" />
</p>

### 原生 Herdr Terminal

在任意 Card 中按 `Cmd+J` 即可并排打开终端分栏，或切到独立的 **Terminals** 工作区管理多分支会话。基于 Rust、`longbridge/nsh`（原生 AppKit / Metal）与 `libghostty-vt` 构建，支持本地与远程 SSH `herdr-core`，重启或升级应用不会中断运行中的 Agent PTY，并内置 17 款暗色主题。

<p align="center">
  <img src="docs/assets/readme-card-terminal.png" width="840" alt="Monokai 主题下的 Card 详情与 Herdr Terminal 分栏视图" />
</p>

---

## 安装

**前置要求**：macOS 13.0+（Apple Silicon）、`git`、至少一个原生 Agent CLI（如 `claude`、`codex`、`agy`、`cursor-agent`），以及 **Herdr Core (`hc`)**（首次启动时若缺失会引导一键安装）。

```bash
curl -fsSL https://canban.dev/install.sh | bash
```

一键安装或热升级 `/Applications/Canban.app`（保留运行中的 `herdr-core` 会话），并在 macOS 上自动清理 Gatekeeper 隔离属性与完成本地签名处理。

---

## 相关软件与致谢

Canban 的实现离不开以下优秀开源项目与资源（完整的第三方版权与许可证文本保留在 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)）：

- **[Herdr (`herdr-core`)](https://github.com/herdrdev/herdr-core)** — 负责 PTY 会话托管、生命周期监督与 JSON-RPC 通信的底层引擎（Apache-2.0）
- **[Sheprd](https://github.com/sheprd-labs/sheprd)** — 在终端 VT/输入封装与订阅工具契约中参考了其工程实践（Apache-2.0）
- **[NSH (`longbridge/nsh`)](https://github.com/longbridge/nsh)** — 原生 macOS AppKit / Metal Rust 桌面 UI 框架（Apache-2.0）
- **[Ghostty (`libghostty-vt`)](https://github.com/ghostty-org/ghostty)** — 按固定提交锁定的高性能 VT 终端解析内核（MIT）
- **[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)** (MIT) — 16 套经典暗色终端配色方案（含 Monokai、Dracula、Nord、Tokyo Night、Catppuccin Mocha 等）
- **[Lucide](https://lucide.dev)** (ISC)、**[Lobe Icons](https://github.com/lobehub/lobe-icons)** (MIT)、**[Primer Octicons](https://github.com/primer/octicons)** (MIT) — UI 图标与模型/服务商标识

---

## 开源协议

Copyright 2026 JJy \<jjyruby@gmail.com\>.

本项目基于 [Apache License, Version 2.0](LICENSE) 开源。
