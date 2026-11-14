/* Copyright 2026 JJy <jjyruby@gmail.com>
 * SPDX-License-Identifier: Apache-2.0
 */

(function () {
  "use strict";

  const INSTALL_COMMANDS = {
    script: "curl -fsSL https://canban.dev/install.sh | sh",
    source: "git clone https://github.com/jjyr/canban.git && cd canban && mise run install-app",
  };

  /* ------------------------------------------------------------------------ */
  /* Localization Dictionary (Default English + Simplified Chinese)           */
  /* Easy to extend: add another locale key (e.g., `ja`) with matching keys.  */
  /* ------------------------------------------------------------------------ */
  const I18N = {
    en: {
      meta_title: "Canban — Kanban + Herdr Workbench for Human Context",
      meta_desc:
        "Run agents in parallel without switching human context. Canban unites an intent-preserving Kanban board, declarative Workflows, and persistent Herdr Terminals.",
      nav_context: "Human Context",
      nav_workbench: "Workbench",
      nav_stack: "Under the Hood",
      nav_install: "Install",
      star_prompt: "Star on GitHub",

      hero_eyebrow: "KANBAN + HERDR WORKBENCH",
      hero_title: "Run agents in parallel.<br>Never switch <em>human context</em>.",
      hero_subtitle:
        "Canban unites an intent-preserving <strong>Kanban</strong> board with persistent <strong>Herdr Terminals</strong>, orchestrated by declarative <strong>Workflows</strong>. Every Card locks in its own discussion, branch, and live session—unburdening your mind.",

      copy_label: "Copy",
      copied_label: "Copied",

      install_platform: "Available for macOS today · Linux & Windows on the roadmap",
      install_license: "Apache-2.0",
      install_source_link: "Or build from source",

      wb_hint_left: "Live Workbench · 4 agents running in parallel across isolated worktrees",
      wb_hint_right: "Click any active Card to switch full context instantly →",
      wb_running_pill: "4 Running",
      wb_attention_pill: "1 Needs Input",

      col_backlog: "Backlog",
      col_develop: "Develop",
      col_parallel_badge: "4 · Parallel",
      col_review: "Review",

      card_68_title: "Linux Wayland window host",
      card_68_sub: "Spec draft · multi-platform",
      card_69_title: "Device OAuth token refresh",
      card_69_sub: "Queued after #63",
      card_70_title: "Warm paper terminal theme",
      card_70_sub: "Palette tokens",

      card_65_title: "Website zen layout & i18n",
      card_65_step: "Layout & motion",
      card_64_title: "Workflow YAML migration v6",
      card_64_step: "cargo test --workspace",
      card_63_title: "GitHub App token rotation",
      card_63_step: "Needs human answer",
      card_62_title: "Worktree archive GC sweep",
      card_62_step: "Verifying clean refs",

      card_61_title: "Durable event bus cursor",
      card_61_sub: "+182 −24 · Queue merge ready",
      card_60_title: "Command palette parity",
      card_60_sub: "+64 −11 · Diff verified",

      status_running: "Running",
      status_needs_input: "Needs Input",
      status_ready: "Ready",

      detail_context_heading: "Human Context & Timeline",
      detail_context_sub: "Zero mental reload",

      ctx_kicker: "HUMAN CONTEXT",
      ctx_title: "Stop acting as the context router between agents.",
      ctx_desc:
        "Parallel agents multiply output, but scattered terminal tabs multiply cognitive load. Canban keeps your mind quiet by anchoring every stream of work to a single Card.",

      contrast_old_title: "Parallelism without Human Context",
      contrast_old_1: "8 scattered terminal tabs—constantly guessing which session belongs to which branch.",
      contrast_old_2: "Re-reading scrollback and re-explaining intent every time you switch tasks.",
      contrast_old_3: "Your brain acts as the state machine, draining focus on routing instead of decisions.",

      contrast_new_title: "Parallel calm in Canban",
      contrast_new_1: "One Card binds intent, timeline discussion, isolated Git worktree, and persistent PTY.",
      contrast_new_2: "Click a Card to recall its complete story and live terminal in under 200ms.",
      contrast_new_3: "Agents advance quietly in the background and surface gentle notices only when your judgment is needed.",

      pillars_kicker: "KANBAN + WORKFLOW + HERDR",
      pillars_title: "Three primitives, one calm workbench.",
      pillars_desc:
        "Built from the ground up as a unified desktop workbench rather than bolted-on scripts.",

      p1_index: "01 · KANBAN",
      p1_title: "Lock in Human Context",
      p1_body:
        "Every Card holds the task brief, @agent discussions, pasted screenshots, and review disposition in one unified Timeline. Switching Cards switches your entire working context cleanly.",

      p2_index: "02 · WORKFLOW",
      p2_title: "Declarative stage flow",
      p2_body:
        "Dragging a Card into Develop or @mentioning a profile dispatches the right agent automatically. Explicit CLI outcome reports advance runs into Review and Queue Merge safely.",

      p3_index: "03 · HERDR TERMINAL",
      p3_title: "Persistent sessions & PTYs",
      p3_body:
        "Powered by Herdr and native libghostty rendering. Closing the window never kills active runs; attach to any live PTY, answer prompts inline, or resume conversations by exact ID.",

      loop_1: "Intent captured",
      loop_2: "Stage & schedule",
      loop_3: "Isolated PTY & worktree",
      loop_4: "Review & Queue Merge",

      stack_kicker: "UNDER THE HOOD",
      stack_title: "Grounded in native systems engineering.",
      stack_desc:
        "Designed for cross-platform longevity—shipping on macOS today, with Linux and Windows on the roadmap.",

      stack_rust_tag: "Core & Engine",
      stack_rust_desc:
        "End-to-end Rust workspace across Kanban, Workflow Engine, Scheduler, and SQLite WAL storage—instant startup and minimal memory footprint.",
      stack_herdr_tag: "Session Daemon",
      stack_herdr_desc:
        "Persistent PTY supervision, live status observation, exact conversation resume, and local or remote machine routing.",
      stack_nsh_tag: "Native UI",
      stack_nsh_desc:
        "Built with NSH UI rather than a heavy web wrapper—delivering crisp 60/120fps responsiveness, keyboard-first Quick Open, and cross-platform evolution.",
      stack_ghostty_tag: "VT & Rendering",
      stack_ghostty_desc:
        "Embeds checksum-pinned libghostty-vt for faithful terminal emulation, low-latency input, and clean ANSI palettes right beside each Card.",

      closing_title: "Unburden your context. Start building.",
      closing_sub: "Install Canban in one command, or build directly from source.",
      tab_script: "Install Script",
      tab_source: "Build from Source",
      cta_star: "Star jjyr/canban on GitHub",
      cta_docs: "Read Architecture Docs",
      footer_copy: "Apache-2.0 License · © 2026 JJy",
    },

    zh: {
      meta_title: "Canban — 面向 Human Context 的 Kanban + Herdr 并行工作台",
      meta_desc:
        "多 Agent 并行推进，无需在会话间频繁切换大脑上下文。Canban 将承载意图的 Kanban 看板、声明式 Workflow 与持久运行的 Herdr Terminal 合二为一。",
      nav_context: "Human Context",
      nav_workbench: "工作台架构",
      nav_stack: "底层技术",
      nav_install: "安装",
      star_prompt: "GitHub 点亮 Star",

      hero_eyebrow: "KANBAN + HERDR WORKBENCH",
      hero_title: "多 Agent 并行推进，<br>无需频繁切换<em>你的大脑上下文</em>。",
      hero_subtitle:
        "Canban 将管理意图的 <strong>Kanban</strong> 与持久运行的 <strong>Herdr Terminal</strong> 合二为一，由声明式 <strong>Workflow</strong> 自动编排推进。每张卡片锁住独立讨论、分支与实时终端，让大脑卸下记忆重担。",

      copy_label: "复制",
      copied_label: "已复制",

      install_platform: "当前首发支持 macOS · Linux 与 Windows 正在规划中",
      install_license: "Apache-2.0 开源协议",
      install_source_link: "或从源码构建",

      wb_hint_left: "实时工作台预览 · 4 个 Agent 正于独立 worktree 中并行开发",
      wb_hint_right: "点击任意卡片，体验零负担秒切完整上下文 →",
      wb_running_pill: "4 并行运行中",
      wb_attention_pill: "1 待人工确认",

      col_backlog: "Backlog",
      col_develop: "Develop",
      col_parallel_badge: "4 · 并行中",
      col_review: "Review",

      card_68_title: "Linux Wayland 原生窗口适配",
      card_68_sub: "方案草案 · 跨平台演进",
      card_69_title: "CLI 设备授权流自动续期",
      card_69_sub: "等待 #63 完成后准入",
      card_70_title: "和纸暖白终端主题色板",
      card_70_sub: "设计 Token 整理",

      card_65_title: "官网 Zen 视觉与多语言设计",
      card_65_step: "正在编写布局与动效",
      card_64_title: "Workflow YAML 迁移契约 v6",
      card_64_step: "正在运行 cargo test",
      card_63_title: "GitHub App 令牌轮换策略",
      card_63_step: "等待人工确认方案",
      card_62_title: "归档 Worktree 自动清理",
      card_62_step: "校验分支引用完整性",

      card_61_title: "持久化事件总线游标恢复",
      card_61_sub: "+182 −24 · 队列合并就绪",
      card_60_title: "全局命令面板快捷键对齐",
      card_60_sub: "+64 −11 · Diff 已核验",

      status_running: "运行中",
      status_needs_input: "待回复",
      status_ready: "就绪",

      detail_context_heading: "Human Context 与时间线",
      detail_context_sub: "切卡即切完整语境",

      ctx_kicker: "HUMAN CONTEXT · 心智留白",
      ctx_title: "告别在十几个 Agent 窗口间充当人肉路由器。",
      ctx_desc:
        "并行 Agent 成倍放大了产出，但散落的终端会话也成倍放大了记忆负担。Canban 把每一条并行工作流锚定在一张卡片上，还大脑一片静谧。",

      contrast_old_title: "缺乏 Human Context 的并行",
      contrast_old_1: "开满 8 个终端 Tab，来回翻找哪个窗口对应哪个分支与任务。",
      contrast_old_2: "每次切换窗口都要重新滚屏回忆前因后果，反复粘贴背景上下文。",
      contrast_old_3: "人脑被迫充当状态机与调度器，精力消耗在找窗口而非关键决策。",

      contrast_new_title: "Canban 的并行秩序",
      contrast_new_1: "一张卡片完整锁定任务意图、讨论时间线、独立 Git worktree 与持久 PTY。",
      contrast_new_2: "点击任意卡片，200ms 内召回全部上下文与实时终端，心智零残留。",
      contrast_new_3: "多个 Agent 在后台静默推进，仅在需要人工拍板或审阅时温和提醒。",

      pillars_kicker: "KANBAN + WORKFLOW + HERDR",
      pillars_title: "三大核心原语，合一静谧工作台。",
      pillars_desc:
        "从第一行代码起即为多 Agent 并行协作原生设计，而非零散脚本拼接。",

      p1_index: "01 · KANBAN",
      p1_title: "锁定 Human Context",
      p1_body:
        "每张卡片收纳任务初衷、@agent 讨论记录、截图资源与审阅结论于统一 Timeline。切换卡片即切换完整工作现场，大脑无需记忆中间状态。",

      p2_index: "02 · WORKFLOW",
      p2_title: "声明式自动流转",
      p2_body:
        "拖入阶段或 @mention 自动唤醒对应角色 Agent；通过显式 CLI 结果汇报驱动阶段流转，安全衔接 Review 审阅与 Queue Merge 队列合并。",

      p3_index: "03 · HERDR TERMINAL",
      p3_title: "持久会话与原生终端",
      p3_body:
        "深度融合 Herdr 守护进程与原生 libghostty 渲染。关闭窗口绝不中断后台任务；随时附着实时 PTY、就地回答追问，或按精确 ID 无损恢复会话。",

      loop_1: "沉淀意图",
      loop_2: "阶段编排与调度",
      loop_3: "独立 Worktree 与持久终端",
      loop_4: "审阅与队列合并",

      stack_kicker: "UNDER THE HOOD · 底层技术",
      stack_title: "纯粹、扎实的原生系统工程底座。",
      stack_desc:
        "面向跨平台长期演进架构设计 —— 当前首发支持 macOS，Linux 与 Windows 支持稳步推进中。",

      stack_rust_tag: "核心与引擎",
      stack_rust_desc:
        "Kanban、Workflow 引擎、调度器与 SQLite WAL 存储全栈采用 Rust 编写，瞬时启动、极低内存占用。",
      stack_herdr_tag: "持久终端底座",
      stack_herdr_desc:
        "负责 PTY 守护、运行态实时观测、会话精确 ID 恢复，以及本地与远程开发机无缝路由。",
      stack_nsh_tag: "原生桌面 UI",
      stack_nsh_desc:
        "基于 NSH UI 构建原生界面，告别笨重浏览器套壳，提供细腻的键盘流 Quick Open 与多平台演进能力。",
      stack_ghostty_tag: "终端解析与渲染",
      stack_ghostty_desc:
        "内嵌校验锁定的 libghostty-vt，在卡片旁提供高帧率、低延迟、色彩温润的原生终端体验。",

      closing_title: "卸下上下文重担，从容开始构建。",
      closing_sub: "一行命令即可安装 Canban，或直接从源码编译体验。",
      tab_script: "一键安装脚本",
      tab_source: "从源码构建",
      cta_star: "在 GitHub 点亮 jjyr/canban",
      cta_docs: "阅读架构文档",
      footer_copy: "Apache-2.0 开源协议 · © 2026 JJy",
    },
  };

  /* Interactive Card Context Data for the Live Workbench Stage */
  const CARD_SCENES = {
    "65": {
      branch: "canban-0afd/65",
      stageClass: "",
      session: "hc attach #65:agy",
      en: {
        stage: "Develop · @agy",
        title: "#65 Website zen layout & i18n",
        timeline: [
          { who: "user", text: "Emphasize Human Context, Kanban + Herdr workbench, and browser auto-i18n.", cls: "" },
          { who: "@agy", text: "Adopted v2 wireframe: 4 parallel Develop cards + interactive context pane.", cls: "is-highlight" },
          { who: "workflow", text: "Stage develop active · isolated worktree clean", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban card show 65 --context" },
          { cls: "term-dim", text: "worktree: ~/.canban/worktrees/canban-0afd/65" },
          { cls: "term-ok", text: "✓ wrote website/index.html, styles.css, app.js" },
          { cls: "", text: "> verifying responsive stage & i18n dictionary", cursor: true },
        ],
      },
      zh: {
        stage: "Develop · @agy",
        title: "#65 官网 Zen 视觉与多语言设计",
        timeline: [
          { who: "user", text: "强调 Human Context 免切上下文，突出 Kanban + Herdr 工作台与浏览器多语言。", cls: "" },
          { who: "@agy", text: "已对齐 v2 线框：Develop 四卡并行预览 + 即点即切上下文面板。", cls: "is-highlight" },
          { who: "workflow", text: "develop 阶段运行中 · 独立 worktree 已挂载", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban card show 65 --context" },
          { cls: "term-dim", text: "worktree: ~/.canban/worktrees/canban-0afd/65" },
          { cls: "term-ok", text: "✓ 已生成 website/index.html, styles.css, app.js" },
          { cls: "", text: "> 正在校验响应式布局与双语无闪烁切换", cursor: true },
        ],
      },
    },
    "64": {
      branch: "canban-0afd/64",
      stageClass: "",
      session: "hc attach #64:codex",
      en: {
        stage: "Develop · @codex",
        title: "#64 Workflow YAML migration v6",
        timeline: [
          { who: "user", text: "Add sequential migration for admit.max_ready defaults in migrations.rs.", cls: "" },
          { who: "@codex", text: "Implemented v5 → v6 step; running round-trip & snapshot tests.", cls: "is-highlight" },
          { who: "workflow", text: "Auto-advance to Review armed on completed report", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ cargo test -p canban-workflow-engine" },
          { cls: "term-ok", text: "test migrations::v5_to_v6_roundtrip ... ok" },
          { cls: "term-ok", text: "test stages::preserves_custom_edges ... ok" },
          { cls: "", text: "running 42 tests across workspace", cursor: true },
        ],
      },
      zh: {
        stage: "Develop · @codex",
        title: "#64 Workflow YAML 迁移契约 v6",
        timeline: [
          { who: "user", text: "在 migrations.rs 中为 admit.max_ready 默认值补充顺序迁移步骤。", cls: "" },
          { who: "@codex", text: "已完成 v5 → v6 迁移实现，正在执行往返兼容与快照回归测试。", cls: "is-highlight" },
          { who: "workflow", text: "收到 completed 汇报后将自动流转至 Review", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ cargo test -p canban-workflow-engine" },
          { cls: "term-ok", text: "test migrations::v5_to_v6_roundtrip ... ok" },
          { cls: "term-ok", text: "test stages::preserves_custom_edges ... ok" },
          { cls: "", text: "正在并行执行其余 42 项工作区测试", cursor: true },
        ],
      },
    },
    "63": {
      branch: "canban-0afd/63",
      stageClass: "amber",
      session: "hc attach #63:claude",
      en: {
        stage: "Needs Input · @claude",
        title: "#63 GitHub App token rotation",
        timeline: [
          { who: "user", text: "Refresh installation tokens secret-free before 5-minute expiry window.", cls: "" },
          {
            who: "@claude ▲",
            text: "Question: Should expired offline tokens pause Queue Merge with `waiting_approval` or retry silently on wake?",
            cls: "is-attention",
          },
          { who: "canban", text: "Card #63 surfaced to Activity pill — reply inline to resume", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban report --state blocked --message 'Awaiting policy choice'" },
          { cls: "term-warn", text: "▲ Agent paused cleanly at decision gate (PTY preserved)" },
          { cls: "term-dim", text: "session id: claude-8f21a · ready for instant @claude reply" },
          { cls: "", text: "? awaiting human answer on Card #63", cursor: true },
        ],
      },
      zh: {
        stage: "待人工确认 · @claude",
        title: "#63 GitHub App 令牌轮换策略",
        timeline: [
          { who: "user", text: "在到期前 5 分钟无密钥刷新 GitHub App 安装令牌。", cls: "" },
          {
            who: "@claude ▲",
            text: "待确认：离线唤醒后若令牌已过期，Queue Merge 应进入 `waiting_approval` 挂起还是静默重试？",
            cls: "is-attention",
          },
          { who: "canban", text: "已置顶温和提醒 —— 在卡片直接回复即可唤醒继续执行", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban report --state blocked --message '等待策略拍板'" },
          { cls: "term-warn", text: "▲ Agent 已在决策节点安全挂起（Herdr PTY 完整保留）" },
          { cls: "term-dim", text: "session id: claude-8f21a · 回复后按精确 ID 无损恢复" },
          { cls: "", text: "? 等待人工回复 Card #63", cursor: true },
        ],
      },
    },
    "62": {
      branch: "canban-0afd/62",
      stageClass: "",
      session: "hc attach #62:codex",
      en: {
        stage: "Develop · @codex",
        title: "#62 Worktree archive GC sweep",
        timeline: [
          { who: "user", text: "Preserve commit under refs/canban/cards/<id> before pruning clean worktrees.", cls: "" },
          { who: "@codex", text: "Verified dirty worktrees stay untouched with explicit notice.", cls: "is-highlight" },
        ],
        term: [
          { cls: "term-cmd", text: "$ git for-each-ref refs/canban/cards/" },
          { cls: "term-dim", text: "9c41e0b commit refs/canban/cards/61" },
          { cls: "term-ok", text: "✓ archive retention invariant verified" },
          { cls: "", text: "> preparing completion report", cursor: true },
        ],
      },
      zh: {
        stage: "Develop · @codex",
        title: "#62 归档 Worktree 自动清理",
        timeline: [
          { who: "user", text: "清理干净 worktree 前将提交快照保留至 refs/canban/cards/<id>。", cls: "" },
          { who: "@codex", text: "已验证含未提交改动的工作树会自动保留并给出提示。", cls: "is-highlight" },
        ],
        term: [
          { cls: "term-cmd", text: "$ git for-each-ref refs/canban/cards/" },
          { cls: "term-dim", text: "9c41e0b commit refs/canban/cards/61" },
          { cls: "term-ok", text: "✓ 归档安全不变量测试通过" },
          { cls: "", text: "> 正在生成任务完成汇报", cursor: true },
        ],
      },
    },
    "61": {
      branch: "canban-0afd/61",
      stageClass: "",
      session: "hc attach #61:merger",
      en: {
        stage: "Review · Queue Merge",
        title: "#61 Durable event bus cursor",
        timeline: [
          { who: "@codex", text: "Completed (+182 −24): durable journal cursor survives daemon restarts.", cls: "is-highlight" },
          { who: "workflow", text: "deliver:review · rebased cleanly onto main, ready to fast-forward.", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban report --state completed --message 'Journal cursor ready'" },
          { cls: "term-ok", text: "✓ Settled turn · rebase onto origin/main clean" },
          { cls: "term-ok", text: "✓ Ready to merge & auto-archive Card #61" },
          { cls: "", text: "idle · session preserved", cursor: true },
        ],
      },
      zh: {
        stage: "Review · 队列合并就绪",
        title: "#61 持久化事件总线游标恢复",
        timeline: [
          { who: "@codex", text: "已完成 (+182 −24)：事件订阅游标跨重启无损恢复。", cls: "is-highlight" },
          { who: "workflow", text: "deliver:review · 已干净变基至 main，准备快进合并与自动归档。", cls: "" },
        ],
        term: [
          { cls: "term-cmd", text: "$ canban report --state completed --message '游标恢复验证通过'" },
          { cls: "term-ok", text: "✓ 会话回合已收尾 · rebase origin/main 无冲突" },
          { cls: "term-ok", text: "✓ 准备合并并自动归档 Card #61" },
          { cls: "", text: "idle · 终端会话持久保留", cursor: true },
        ],
      },
    },
  };

  let currentLang = document.documentElement.getAttribute("data-lang") || "en";
  if (!I18N[currentLang]) currentLang = "en";

  let activeCardId = "65";
  let activeInstallTab = "script";
  let autoCycleTimer = null;
  let userInteractedWithStage = false;

  function applyLanguage(lang, persist) {
    const resolved = I18N[lang] ? lang : "en";
    currentLang = resolved;
    document.documentElement.setAttribute("lang", resolved === "zh" ? "zh-CN" : "en");
    document.documentElement.setAttribute("data-lang", resolved);

    if (persist) {
      try {
        localStorage.setItem("canban_lang", resolved);
      } catch (_) {
        /* ignore private mode storage restrictions */
      }
    }

    const dict = I18N[resolved];
    document.title = dict.meta_title;
    const metaDesc = document.querySelector('meta[name="description"]');
    if (metaDesc) metaDesc.setAttribute("content", dict.meta_desc);

    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      if (dict[key] !== undefined) {
        el.textContent = dict[key];
      }
    });

    document.querySelectorAll("[data-i18n-html]").forEach((el) => {
      const key = el.getAttribute("data-i18n-html");
      if (dict[key] !== undefined) {
        el.innerHTML = dict[key];
      }
    });

    document.querySelectorAll(".lang-btn").forEach((btn) => {
      const isMatch = btn.getAttribute("data-set-lang") === resolved;
      btn.setAttribute("aria-pressed", isMatch ? "true" : "false");
    });

    renderCardDetail(activeCardId, false);
  }

  function renderCardDetail(cardId, animate) {
    const scene = CARD_SCENES[cardId] || CARD_SCENES["65"];
    const localeData = scene[currentLang] || scene.en;
    const detailPane = document.getElementById("wb-detail");
    if (!detailPane) return;

    document.querySelectorAll("[data-card-id]").forEach((cardBtn) => {
      const selected = cardBtn.getAttribute("data-card-id") === cardId;
      cardBtn.classList.toggle("is-selected", selected);
      cardBtn.setAttribute("aria-selected", selected ? "true" : "false");
    });

    const updateDOM = () => {
      const branchEl = document.getElementById("detail-branch");
      const stageEl = document.getElementById("detail-stage");
      const titleEl = document.getElementById("detail-title");
      const timelineEl = document.getElementById("detail-timeline");
      const sessionEl = document.getElementById("detail-session");
      const termBodyEl = document.getElementById("detail-term-body");

      if (branchEl) branchEl.textContent = scene.branch;
      if (stageEl) {
        stageEl.textContent = localeData.stage;
        stageEl.className = "wb-stage-pill" + (scene.stageClass ? " " + scene.stageClass : "");
      }
      if (titleEl) titleEl.textContent = localeData.title;
      if (sessionEl) sessionEl.textContent = scene.session;

      if (timelineEl) {
        timelineEl.innerHTML = "";
        localeData.timeline.forEach((item) => {
          const row = document.createElement("div");
          row.className = "wb-tl-item" + (item.cls ? " " + item.cls : "");
          const who = document.createElement("span");
          who.className = "wb-tl-who";
          who.textContent = item.who;
          const txt = document.createElement("span");
          txt.className = "wb-tl-text";
          txt.textContent = item.text;
          row.appendChild(who);
          row.appendChild(txt);
          timelineEl.appendChild(row);
        });
      }

      if (termBodyEl) {
        termBodyEl.innerHTML = "";
        localeData.term.forEach((line) => {
          const div = document.createElement("div");
          div.className = "term-line" + (line.cls ? " " + line.cls : "");
          div.appendChild(document.createTextNode(line.text));
          if (line.cursor) {
            const cur = document.createElement("span");
            cur.className = "term-cursor";
            cur.setAttribute("aria-hidden", "true");
            div.appendChild(cur);
          }
          termBodyEl.appendChild(div);
        });
      }
    };

    if (animate && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      detailPane.classList.add("is-switching");
      window.setTimeout(() => {
        updateDOM();
        detailPane.classList.remove("is-switching");
      }, 110);
    } else {
      updateDOM();
    }
  }

  function startAmbientStageCycle() {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const order = ["65", "64", "63", "62", "61"];
    autoCycleTimer = window.setInterval(() => {
      if (userInteractedWithStage || document.hidden) return;
      const idx = order.indexOf(activeCardId);
      activeCardId = order[(idx + 1) % order.length];
      renderCardDetail(activeCardId, true);
    }, 5200);
  }

  function copyText(text, buttonEl) {
    const dict = I18N[currentLang] || I18N.en;
    const labelEl = buttonEl.querySelector(".copy-text");
    const iconCopy = buttonEl.querySelector(".icon-copy");
    const iconCheck = buttonEl.querySelector(".icon-check");

    const showSuccess = () => {
      buttonEl.classList.add("is-copied");
      if (labelEl) labelEl.textContent = dict.copied_label;
      if (iconCopy) iconCopy.style.display = "none";
      if (iconCheck) iconCheck.style.display = "inline-block";

      window.clearTimeout(buttonEl._copyTimer);
      buttonEl._copyTimer = window.setTimeout(() => {
        buttonEl.classList.remove("is-copied");
        const latestDict = I18N[currentLang] || I18N.en;
        if (labelEl) labelEl.textContent = latestDict.copy_label;
        if (iconCopy) iconCopy.style.display = "";
        if (iconCheck) iconCheck.style.display = "none";
      }, 2000);
    };

    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(showSuccess).catch(() => fallbackCopy(text, showSuccess));
    } else {
      fallbackCopy(text, showSuccess);
    }
  }

  function fallbackCopy(text, onDone) {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
      onDone();
    } catch (_) {
      /* ignore */
    }
    document.body.removeChild(ta);
  }

  function setupInstallTabs() {
    const tabs = document.querySelectorAll("[data-install-tab]");
    const footerCmdEl = document.getElementById("closing-install-cmd");
    const footerCopyBtn = document.getElementById("closing-copy-btn");

    tabs.forEach((tab) => {
      tab.addEventListener("click", () => {
        const mode = tab.getAttribute("data-install-tab");
        if (!INSTALL_COMMANDS[mode]) return;
        activeInstallTab = mode;
        tabs.forEach((t) =>
          t.setAttribute("aria-selected", t.getAttribute("data-install-tab") === mode ? "true" : "false")
        );
        if (footerCmdEl) footerCmdEl.textContent = INSTALL_COMMANDS[mode];
        if (footerCopyBtn) footerCopyBtn.setAttribute("data-copy-value", INSTALL_COMMANDS[mode]);
      });
    });

    const jumpSourceLink = document.getElementById("jump-build-source");
    if (jumpSourceLink) {
      jumpSourceLink.addEventListener("click", () => {
        const sourceTab = document.querySelector('[data-install-tab="source"]');
        if (sourceTab) sourceTab.click();
      });
    }
  }

  function setupRevealObserver() {
    const items = document.querySelectorAll(".reveal");
    if (!("IntersectionObserver" in window)) {
      items.forEach((el) => el.classList.add("is-visible"));
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-visible");
            io.unobserve(entry.target);
          }
        });
      },
      { threshold: 0.1, rootMargin: "0px 0px -40px 0px" }
    );
    items.forEach((el) => io.observe(el));
  }

  function fetchGitHubStars() {
    const countEls = document.querySelectorAll(".star-count");
    if (!countEls.length) return;
    fetch("https://api.github.com/repos/jjyr/canban", {
      headers: { Accept: "application/vnd.github+json" },
    })
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => {
        if (!data || typeof data.stargazers_count !== "number" || data.stargazers_count < 1) return;
        const formatted =
          data.stargazers_count >= 1000
            ? (data.stargazers_count / 1000).toFixed(1).replace(/\.0$/, "") + "k"
            : String(data.stargazers_count);
        countEls.forEach((el) => {
          el.textContent = formatted;
        });
      })
      .catch(() => {
        /* Graceful silent fallback when repo is private or offline */
      });
  }

  document.addEventListener("DOMContentLoaded", () => {
    /* Language switchers */
    document.querySelectorAll("[data-set-lang]").forEach((btn) => {
      btn.addEventListener("click", () => {
        applyLanguage(btn.getAttribute("data-set-lang"), true);
      });
    });

    /* Interactive Kanban cards in Product Stage */
    document.querySelectorAll("[data-card-id]").forEach((cardBtn) => {
      cardBtn.addEventListener("click", () => {
        userInteractedWithStage = true;
        if (autoCycleTimer) window.clearInterval(autoCycleTimer);
        const nextId = cardBtn.getAttribute("data-card-id");
        if (nextId === activeCardId) return;
        activeCardId = nextId;
        renderCardDetail(activeCardId, true);
      });
    });

    /* Copy command buttons */
    document.querySelectorAll("[data-copy-btn]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const val = btn.getAttribute("data-copy-value") || INSTALL_COMMANDS[activeInstallTab];
        copyText(val, btn);
      });
    });

    setupInstallTabs();
    applyLanguage(currentLang, false);
    setupRevealObserver();
    startAmbientStageCycle();
    fetchGitHubStars();
  });
})();
