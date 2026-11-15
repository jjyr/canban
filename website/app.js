/* Copyright 2026 JJy <jjyruby@gmail.com>
 * SPDX-License-Identifier: Apache-2.0
 */

(function () {
  "use strict";

  const INSTALL_COMMANDS = {
    script: "curl -fsSL https://canban.dev/install.sh | bash",
    source: "git clone https://github.com/jjyr/canban.git && cd canban && mise run install-app",
  };

  /* ------------------------------------------------------------------------ */
  /* Bilingual Copy                                                           */
  /* ------------------------------------------------------------------------ */
  const I18N = {
    en: {
      meta_title:
        "Canban — AI-Native Kanban Workbench for Coding Agents (Kanban + Workflow + Herdr)",
      meta_desc:
        "Canban is an open-source native macOS Kanban workbench for coding agents. Maintain Human Context on Kanban cards, automate isolated Git worktrees & Queue Merge with custom Workflows, and run Claude Code, Codex, or Agy inside Herdr persistent PTY terminals.",

      ch_1: "WORKBENCH",
      ch_2: "CONTEXT",
      ch_3: "TOPOLOGY",
      ch_4: "INSTALL",
      star_prompt: "STAR",

      hero_subtitle:
        "Maintain Human Context with Kanban, schedule with custom Workflows, and power native Agent CLIs with Herdr.",

      tab_1_desc: "Maintain Human Context",
      tab_2_desc: "Custom Workflow Scheduling",
      tab_3_desc: "Native Agent CLI Support",

      term_chrome_crumb: "canban / Card #65 · CRT & Monokai polish",
      c68_title: "Wayland fractional scale host",
      c67_title: "Remote machine SSH multiplexer",
      c65_title: "CRT deck & Monokai polish",
      c63_title: "GitHub App token rotation",
      c63_wait: "Needs answer",
      c64_title: "Workflow schema v7 migration",
      c61_title: "Durable event journal cursor",
      c60_title: "Unified ⌘O Command Palette",

      tl_msg_1: "Build a bold retro CRT Monokai deck, one screen per channel.",
      tl_mile: "⎇ Worktree <code>canban-0afd/65</code> · @developer started",
      tl_msg_2: "Redrew full width Workflow Studio and live Agent CLI view.",

      wf_intake: "Intake",
      wn_0_title: "Plan task",
      wn_0_sub: "Inspect repository & draft implementation steps",
      wn_1_title: "Wait for free Agent",
      wn_1_sub: "Admit in Board order when global slot is free",
      wn_2_title: "Implement",
      wn_2_sub: "Isolated Git worktree · awaits explicit CLI report",
      wn_3_title: "External PR check",
      wn_4_title: "Queue merge",
      wn_4_sub: "FIFO batch integration & upstream branch sync",

      hs_desc_label: "Description",
      hs_desc_text:
        "Present Canban inside a retro Monokai CRT deck. Keep Workflow pure visual Studio and show real Agent CLI TUI beside the Card.",
      hs_comment_1: "@developer Add CRT scanlines, channel glitch, and ASCII topology on CH.03.",
      hs_comment_2: "Applying Monokai phosphor tokens and verifying layout.",
      tsc_placeholder: "Write a comment…",
      tui_prompt_hint: "Steer @developer directly in TTY, or close window safely…",

      ctx_title: "Track all task context and progress with Kanban cards.",
      old_tag: "Scattered Terminal Tabs",
      new_tag: "1 Card = Complete Context",
      ccm_title: "GitHub App device OAuth token refresh",
      ccm_msg_user:
        "Refresh installation tokens 5m before expiry without storing client secrets. @developer",
      ccm_msg_agent:
        "Implemented secretless Device Flow rotation and passed 14 auth integration tests. Ready to merge.",
      ccm_mile: "✓ Queued for automatic rebase & upstream sync",

      ascii_title: "Parallel Agents. Isolated Worktrees. Zero Conflict Delivery.",
      rf_card_title: "Token Refresh & Sync",
      rf_plan_title: "Plan & Admit",
      al_1: "Zero branch collisions: every Card gets its own clean checkout",
      al_2: "Bring any native Agent CLI inside persistent Herdr TTYs",
      al_3: "Automatic FIFO rebase merge, upstream push & worktree cleanup",

      closing_title: "Unburden your mind. Keep your Human Context.",
      tab_script: "Install Script",
      tab_source: "Build from Source",
      copy_label: "Copy",
      copied_label: "Copied",
      cta_star: "Star jjyr/canban on GitHub",
      cta_docs: "Architecture Docs →",

      s_rust: "Canban Core Engine",
      s_herdr: "TTY Agent Support",
      s_nsh: "Native UI",
      s_ghostty: "Terminal Rendering Engine",
      closing_seo_note:
        "Canban is an open-source native macOS Kanban workbench for AI coding agents — built in Rust to run Claude Code, Codex, Antigravity (agy), and Cursor CLI in isolated Git worktrees with persistent Herdr PTY terminals.",

      footer_meta: "Apache-2.0 · © 2026 JJy",
    },

    zh: {
      meta_title:
        "Canban — 面向 AI 编程 Agent 的原生 Kanban 工作台 (Kanban + Workflow + Herdr)",
      meta_desc:
        "Canban 是开源原生 macOS 编程 Agent 看板工作台：用 Kanban 卡片维护 Human Context，通过自定义 Workflow 自动隔离 Git Worktree 与 Queue Merge，并在 Herdr 持久化 PTY 终端中直接运行 Claude Code、Codex、Agy 等原生 Agent CLI。",

      ch_1: "工作台",
      ch_2: "上下文",
      ch_3: "并行拓扑",
      ch_4: "安装",
      star_prompt: "STAR",

      hero_subtitle:
        "用 Kanban 维护 Human Context，自定义 Workflow 调度，Herdr 提供原生的 Agent CLI 支持。",

      tab_1_desc: "维护 Human Context",
      tab_2_desc: "自定义 Workflow 调度",
      tab_3_desc: "原生的 Agent CLI 支持",

      term_chrome_crumb: "canban / Card #65 · CRT 复古终端与 Monokai 质感",
      c68_title: "Wayland 分数缩放窗口适配",
      c67_title: "远程机器 SSH 多路复用连接",
      c65_title: "CRT 复古终端与 Monokai 质感",
      c63_title: "GitHub App 令牌轮换机制",
      c63_wait: "待人工回复",
      c64_title: "Workflow schema v7 迁移",
      c61_title: "持久化事件日志游标恢复",
      c60_title: "统一 ⌘O 全局命令面板",

      tl_msg_1: "采用老式 CRT 终端切屏风格，搭配 Monokai 荧光质感。",
      tl_mile: "⎇ Worktree <code>canban-0afd/65</code> · @developer 已启动",
      tl_msg_2: "已重绘全宽 Workflow Studio 与右侧真实 Agent CLI 界面。",

      wf_intake: "准入",
      wn_0_title: "任务规划",
      wn_0_sub: "读取仓库结构并生成实施步骤",
      wn_1_title: "等待空闲 Agent 槽位",
      wn_1_sub: "按看板优先级排队，有空闲并发槽位自动准入",
      wn_2_title: "编码实现",
      wn_2_sub: "独立 Git Worktree 隔离开发 · 等待 CLI 显式汇报",
      wn_3_title: "外部 PR 安全检查",
      wn_4_title: "Queue Merge 队列合并",
      wn_4_sub: "FIFO 批量变基集成并自动推送远端分支",

      hs_desc_label: "任务描述",
      hs_desc_text:
        "采用复古 CRT 显像管分屏展示 Canban。Workflow 保持纯图形化 Studio，Herdr 分屏展示真实 Agent CLI 交互输出。",
      hs_comment_1: "@developer 加上 CRT 扫描线噪点、切台 Glitch 和第 3 屏 ASCII 拓扑图。",
      hs_comment_2: "正在写入 Monokai 荧光色板并校验各频道布局。",
      tsc_placeholder: "输入评论或 @agent…",
      tui_prompt_hint: "直接在 TTY 中引导 @developer，或随时安全关闭窗口…",

      ctx_title: "使用 Kanban 卡片追踪任务相关的所有上下文以及进度",
      old_tag: "散落的终端标签",
      new_tag: "1 张 Card = 完整现场",
      ccm_title: "GitHub App 设备流 OAuth 令牌无感续期",
      ccm_msg_user:
        "在过期前 5 分钟自动刷新安装令牌，本地不落盘任何 Client Secret。@developer",
      ccm_msg_agent:
        "已完成免密钥 Device Flow 轮换逻辑，14 项鉴权集成测试全部通过，可随时合入。",
      ccm_mile: "✓ 已进入 Queue Merge 自动变基合并队列",

      ascii_title: "多 Agent 并行 · 独立 Worktree 隔离 · 零冲突自动合入",
      rf_card_title: "令牌无感刷新与同步",
      rf_plan_title: "规划与准入",
      al_1: "零分支污染：每张 Card 独占干净的 Git Worktree",
      al_2: "不绑架工具：在 Herdr 持久 TTY 中运行任意原生 Agent CLI",
      al_3: "自动交付闭环：FIFO 批量变基合并、远端同步并自动清理归档",

      closing_title: "卸下思维负担，保持 Human Context。",
      tab_script: "一键脚本安装",
      tab_source: "从源码构建",
      copy_label: "复制",
      copied_label: "已复制",
      cta_star: "在 GitHub 点亮 Star",
      cta_docs: "阅读架构文档 →",

      s_rust: "Canban 核心引擎",
      s_herdr: "提供 TTY Agent 支持",
      s_nsh: "Native UI",
      s_ghostty: "终端渲染引擎",
      closing_seo_note:
        "Canban 是基于 Rust 构建的开源原生 macOS 编程 Agent 看板工作台，支持在独立 Git Worktree 与 Herdr 持久 PTY 终端中运行 Claude Code、Codex、Antigravity (agy) 与 Cursor CLI。",

      footer_meta: "Apache-2.0 · © 2026 JJy",
    },
  };

  /* ------------------------------------------------------------------------ */
  /* Channel Paging Deck (One Screen at a Time + CRT Glitch Transition)       */
  /* ------------------------------------------------------------------------ */
  const TOTAL_CHANNELS = 4;
  const CHANNEL_OSD_NAMES = [
    "CH 01 // WORKBENCH",
    "CH 02 // HUMAN CONTEXT",
    "CH 03 // PIPELINE",
    "CH 04 // INSTALL",
  ];

  let currentChannel = 0;
  let wheelLockUntil = 0;
  let wheelAccum = 0;
  let osdTimer = null;
  let touchStartY = null;

  function switchChannel(nextIndex, syncHash) {
    const clamped = Math.max(0, Math.min(TOTAL_CHANNELS - 1, nextIndex));
    if (clamped === currentChannel) return;

    currentChannel = clamped;

    if (syncHash !== false && window.history && window.history.replaceState) {
      const hash = currentChannel === 0 ? window.location.pathname + window.location.search : `#ch-${currentChannel}`;
      try {
        window.history.replaceState(null, "", hash);
      } catch (_) {}
    }

    const glass = document.getElementById("crt-glass");
    const osd = document.getElementById("crt-osd");
    const readout = document.getElementById("ch-readout");

    /* Trigger CRT H-Sync Glitch Burst */
    if (glass && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      glass.classList.remove("is-glitching");
      void glass.offsetWidth;
      glass.classList.add("is-glitching");
      window.setTimeout(() => glass.classList.remove("is-glitching"), 240);
    }

    /* Update active screen */
    document.querySelectorAll(".crt-screen").forEach((sec) => {
      const idx = Number(sec.getAttribute("data-ch"));
      sec.classList.toggle("is-active", idx === currentChannel);
    });

    /* Update top channel buttons */
    document.querySelectorAll("[data-goto-ch]").forEach((btn) => {
      if (btn.classList.contains("ch-btn")) {
        const idx = Number(btn.getAttribute("data-goto-ch"));
        btn.classList.toggle("is-active", idx === currentChannel);
      }
    });

    /* Update bottom readout */
    if (readout) {
      readout.textContent = `CH 0${currentChannel + 1} / 0${TOTAL_CHANNELS}`;
    }

    /* Flash retro green OSD label */
    if (osd) {
      osd.textContent = CHANNEL_OSD_NAMES[currentChannel];
      osd.classList.add("is-shown");
      window.clearTimeout(osdTimer);
      osdTimer = window.setTimeout(() => osd.classList.remove("is-shown"), 1150);
    }
  }

  function setupChannelNavigation() {
    const applyHashChannel = () => {
      const m = /^#ch-([0-3])$/.exec(window.location.hash || "");
      if (m) switchChannel(Number(m[1]), false);
    };
    applyHashChannel();
    window.addEventListener("hashchange", applyHashChannel);

    document.querySelectorAll("[data-goto-ch]").forEach((el) => {
      el.addEventListener("click", (e) => {
        e.preventDefault();
        const target = Number(el.getAttribute("data-goto-ch"));
        switchChannel(target);
      });
    });

    const prevBtn = document.getElementById("prev-ch-btn");
    const nextBtn = document.getElementById("next-ch-btn");
    if (prevBtn) prevBtn.addEventListener("click", () => switchChannel(currentChannel - 1));
    if (nextBtn) nextBtn.addEventListener("click", () => switchChannel(currentChannel + 1));

    /* Wheel paging (one discrete channel jump per gesture) */
    window.addEventListener(
      "wheel",
      (e) => {
        const now = performance.now();
        if (now < wheelLockUntil) return;

        /* Allow internal scroll if screen content overflows on tiny viewports */
        const activeScreen = document.querySelector(".crt-screen.is-active");
        if (activeScreen && activeScreen.scrollHeight > activeScreen.clientHeight + 8) {
          const atTop = activeScreen.scrollTop <= 2;
          const atBottom =
            activeScreen.scrollTop + activeScreen.clientHeight >= activeScreen.scrollHeight - 2;
          if ((e.deltaY > 0 && !atBottom) || (e.deltaY < 0 && !atTop)) {
            return;
          }
        }

        wheelAccum += e.deltaY;
        if (Math.abs(wheelAccum) >= 42) {
          const dir = wheelAccum > 0 ? 1 : -1;
          wheelAccum = 0;
          wheelLockUntil = now + 560;
          switchChannel(currentChannel + dir);
        }
      },
      { passive: true }
    );

    /* Touch swipe paging */
    window.addEventListener(
      "touchstart",
      (e) => {
        if (e.touches.length === 1) touchStartY = e.touches[0].clientY;
      },
      { passive: true }
    );
    window.addEventListener(
      "touchend",
      (e) => {
        if (touchStartY === null || !e.changedTouches.length) return;
        const dy = touchStartY - e.changedTouches[0].clientY;
        touchStartY = null;
        if (Math.abs(dy) > 48) {
          switchChannel(currentChannel + (dy > 0 ? 1 : -1));
        }
      },
      { passive: true }
    );

    /* Keyboard ArrowUp/Down, PageUp/PageDown, 1-4 */
    window.addEventListener("keydown", (e) => {
      if (e.target && ["INPUT", "TEXTAREA"].includes(e.target.tagName)) return;
      if (e.key === "ArrowDown" || e.key === "PageDown" || e.key === " ") {
        e.preventDefault();
        switchChannel(currentChannel + 1);
      } else if (e.key === "ArrowUp" || e.key === "PageUp") {
        e.preventDefault();
        switchChannel(currentChannel - 1);
      } else if (["1", "2", "3", "4"].includes(e.key) && !e.metaKey && !e.ctrlKey) {
        switchChannel(Number(e.key) - 1);
      }
    });
  }

  /* ------------------------------------------------------------------------ */
  /* CH 01 Scene Sequencer: Kanban | Workflow | Herdr                         */
  /* ------------------------------------------------------------------------ */
  const SCENES = ["kanban", "workflow", "terminal"];
  const SCENE_DURATION_MS = 8600;

  let currentLang = document.documentElement.getAttribute("data-lang") === "zh" ? "zh" : "en";
  let activeInstallTab = "script";
  let activeScene = "kanban";
  let sceneStartTime = performance.now();
  let rafId = null;
  let sceneTimers = [];

  function clearSceneTimers() {
    while (sceneTimers.length) {
      window.clearTimeout(sceneTimers.pop());
    }
  }

  function scheduleInScene(fn, delayMs) {
    const id = window.setTimeout(fn, delayMs);
    sceneTimers.push(id);
    return id;
  }

  /* Scene 01: Kanban Card Click -> Slide Open Pure Card Reader (No Bottom Terminal) */
  function playKanbanScene(reduced) {
    const ws = document.getElementById("kanban-workspace");
    const card = document.getElementById("target-card-65");
    const cursor = document.getElementById("kanban-cursor");
    const replyNode = document.getElementById("kanban-live-reply");
    if (!ws || !card || !cursor) return;

    if (reduced) {
      cursor.className = "sim-cursor";
      card.classList.add("is-card-selected");
      ws.classList.add("is-drawer-open");
      if (replyNode) replyNode.classList.add("is-visible");
      return;
    }

    ws.classList.remove("is-drawer-open");
    card.classList.remove("is-card-selected");
    if (replyNode) replyNode.classList.remove("is-visible");
    cursor.className = "sim-cursor";

    scheduleInScene(() => cursor.classList.add("is-visible", "is-aimed"), 260);
    scheduleInScene(() => {
      cursor.classList.add("is-clicking");
      card.classList.add("is-card-selected");
      ws.classList.add("is-drawer-open");
    }, 1020);
    scheduleInScene(() => cursor.classList.remove("is-visible", "is-clicking"), 1500);
    scheduleInScene(() => {
      if (replyNode) replyNode.classList.add("is-visible");
    }, 2400);
  }

  /* Scene 02: Full-Width Visual Workflow Studio Traversal */
  const WF_TRAVERSAL = [
    { node: 0, en: "1/5 · Backlog · @planner drafting steps", zh: "1/5 · Backlog · @planner 生成实施步骤" },
    { node: 1, vwire: 0, en: "2/5 · Backlog · Free Agent slot granted (4/6)", zh: "2/5 · Backlog · 获得空闲并发槽位 (4/6)" },
    { node: 2, wire: 0, en: "3/5 · Develop · @developer running in worktree", zh: "3/5 · Develop · @developer 在独立分支编码" },
    { node: 3, wire: 1, en: "4/5 · Review · Trusted branch verified", zh: "4/5 · Review · 本地受信任分支校验通过" },
    { node: 4, vwire: 1, en: "5/5 · Review · Queue Merge Rebase ×5 & Sync ✓", zh: "5/5 · Review · Queue Merge 批量变基同步 ✓" },
  ];

  function playWorkflowScene(reduced) {
    const nodes = document.querySelectorAll("[data-wf-node]");
    const wires = document.querySelectorAll("[data-wf-wire]");
    const arrows = document.querySelectorAll("[data-wf-arrow]");
    const vwires = document.querySelectorAll("[data-wf-vwire]");
    const banner = document.getElementById("wf-step-banner");

    nodes.forEach((n) => n.classList.remove("is-lit", "is-done"));
    wires.forEach((w) => w.classList.remove("is-lit"));
    arrows.forEach((a) => a.classList.remove("is-lit"));
    vwires.forEach((v) => v.classList.remove("is-lit"));

    const stepTo = (idx) => {
      const s = WF_TRAVERSAL[idx];
      if (!s) return;
      nodes.forEach((n) => {
        const k = Number(n.getAttribute("data-wf-node"));
        n.classList.toggle("is-lit", k === s.node);
        n.classList.toggle("is-done", k < s.node);
      });
      if (typeof s.wire === "number") {
        const w = document.querySelector(`[data-wf-wire="${s.wire}"]`);
        const a = document.querySelector(`[data-wf-arrow="${s.wire}"]`);
        if (w) w.classList.add("is-lit");
        if (a) a.classList.add("is-lit");
      }
      if (typeof s.vwire === "number") {
        const vw = document.querySelector(`[data-wf-vwire="${s.vwire}"]`);
        if (vw) vw.classList.add("is-lit");
      }
      if (banner) banner.textContent = currentLang === "zh" ? s.zh : s.en;
    };

    if (reduced) {
      WF_TRAVERSAL.forEach((_, i) => stepTo(i));
      return;
    }

    [200, 1650, 3250, 4850, 6350].forEach((ms, i) => {
      scheduleInScene(() => stepTo(i), ms);
    });
  }

  /* Scene 03: Herdr — Authentic Agent CLI TUI Stream (Thoughts + Tool Calls + Diff) */
  function playTerminalScene(reduced) {
    const container = document.getElementById("agent-tui-stream");
    if (!container) return;
    container.innerHTML = "";

    const blocks = [
      {
        delay: 180,
        html: `<div class="tui-tool-head"><span class="tui-bullet is-ok">●</span><span class="tui-tool-name">Read</span><span class="tui-tool-arg">(crates/themes/src/lib.rs#L94-L103)</span></div>
               <div class="tui-thought vt-dim">⎿ Loaded Monokai 16-color ANSI palette &amp; chrome tokens</div>`,
      },
      {
        delay: 1550,
        html: `<div class="tui-tool-head"><span class="tui-bullet is-ok">●</span><span class="tui-tool-name">Edit</span><span class="tui-tool-arg">(website/styles.css)</span></div>
               <div class="tui-diff">
                 <div class="diff-del">- --bg: #FAF9F6; /* warm paper */</div>
                 <div class="diff-add">+ --tube-bg: #151612; /* Monokai CRT phosphor */</div>
                 <div class="diff-add">+ --green: #a6e22e; --pink: #f92672; --cyan: #66d9ef;</div>
               </div>`,
      },
      {
        delay: 3400,
        html: `<div class="tui-tool-head"><span class="tui-bullet">✦</span><span class="vt-purple">Agent reasoning</span></div>
               <div class="tui-thought">${
                 currentLang === "zh"
                   ? "已移除左侧冗余 YAML 栏，将右侧切换为原生 Agent TUI 实时视图。"
                   : "Removed redundant YAML sidebar and wired live Agent TUI stream."
               }</div>`,
      },
      {
        delay: 5300,
        html: `<div class="tui-tool-head"><span class="tui-bullet is-ok">●</span><span class="tui-tool-name">Bash</span><span class="tui-tool-arg">(canban report --state completed)</span></div>
               <div class="tui-thought vt-green">⎿ ✓ Reported completed to AppServer → advancing Card #65 to Review</div>`,
      },
    ];

    const appendBlock = (b) => {
      const div = document.createElement("div");
      div.className = "tui-block";
      div.innerHTML = b.html;
      container.appendChild(div);
    };

    if (reduced) {
      blocks.forEach(appendBlock);
      return;
    }

    blocks.forEach((b) => {
      scheduleInScene(() => appendBlock(b), b.delay);
    });
  }

  function activateScene(sceneName) {
    if (!SCENES.includes(sceneName)) return;
    clearSceneTimers();
    activeScene = sceneName;
    sceneStartTime = performance.now();

    const win = document.getElementById("app-window");
    if (win) win.setAttribute("data-active-scene", sceneName);

    document.querySelectorAll(".feature-tab").forEach((tab) => {
      const match = tab.getAttribute("data-scene") === sceneName;
      tab.classList.toggle("is-active", match);
      tab.setAttribute("aria-selected", match ? "true" : "false");
    });

    SCENES.forEach((s) => {
      const bar = document.getElementById("prog-" + s);
      if (bar) bar.style.width = "0%";
      const el = document.getElementById("scene-" + s);
      if (el) el.classList.toggle("is-active", s === sceneName);
    });

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (sceneName === "kanban") playKanbanScene(reduced);
    else if (sceneName === "workflow") playWorkflowScene(reduced);
    else if (sceneName === "terminal") playTerminalScene(reduced);
  }

  function startLoops() {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    /* Continuous progress bar & auto-advance on CH 01 */
    const tick = (now) => {
      if (document.hidden || currentChannel !== 0) {
        sceneStartTime = now;
        rafId = window.requestAnimationFrame(tick);
        return;
      }
      const elapsed = now - sceneStartTime;
      const pct = Math.min(100, (elapsed / SCENE_DURATION_MS) * 100);
      const bar = document.getElementById("prog-" + activeScene);
      if (bar) bar.style.width = pct.toFixed(1) + "%";

      if (elapsed >= SCENE_DURATION_MS) {
        const nextIdx = (SCENES.indexOf(activeScene) + 1) % SCENES.length;
        activateScene(SCENES[nextIdx]);
      }
      rafId = window.requestAnimationFrame(tick);
    };
    rafId = window.requestAnimationFrame(tick);

    /* Subtle periodic micro-glitch on Hero Formula + Retro Flow stage pulse on CH 03 */
    let pulseStep = 0;
    window.setInterval(() => {
      if (document.hidden) return;
      pulseStep = (pulseStep + 1) % 4;
      document.querySelectorAll("[data-rf-step]").forEach((el) => {
        el.classList.toggle("is-hot", Number(el.getAttribute("data-rf-step")) === pulseStep);
      });
      document.querySelectorAll("[data-rf-wire]").forEach((el) => {
        el.classList.toggle("is-hot", Number(el.getAttribute("data-rf-wire")) === pulseStep - 1);
      });

      if (Math.random() < 0.35) {
        const formula = document.querySelector(".hero-formula");
        if (formula) {
          formula.classList.add("is-micro-glitch");
          window.setTimeout(() => formula.classList.remove("is-micro-glitch"), 110);
        }
      }
    }, 950);
  }

  /* ------------------------------------------------------------------------ */
  /* Localization & Copy Helpers                                              */
  /* ------------------------------------------------------------------------ */
  function applyLanguage(lang, persist) {
    const normalized = lang === "zh" ? "zh" : "en";
    currentLang = normalized;
    const dict = I18N[normalized];

    document.documentElement.setAttribute("data-lang", normalized);
    document.documentElement.setAttribute("lang", normalized === "zh" ? "zh-CN" : "en");
    document.title = dict.meta_title;

    const metaDesc = document.querySelector('meta[name="description"]');
    if (metaDesc) metaDesc.setAttribute("content", dict.meta_desc);

    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      if (dict[key] !== undefined) {
        if (key === "tl_mile") {
          el.innerHTML = dict[key];
        } else {
          el.textContent = dict[key];
        }
      }
    });

    document.querySelectorAll("[data-set-lang]").forEach((btn) => {
      btn.setAttribute("aria-pressed", btn.getAttribute("data-set-lang") === normalized ? "true" : "false");
    });

    if (persist) {
      try {
        localStorage.setItem("canban_lang", normalized);
      } catch (_) {}
      if (window.history && window.history.replaceState) {
        try {
          const url = new URL(window.location.href);
          if (normalized === "zh") {
            url.searchParams.set("lang", "zh");
          } else {
            url.searchParams.delete("lang");
          }
          window.history.replaceState(null, "", url.pathname + url.search + url.hash);
        } catch (_) {}
      }
    }
  }

  function copyText(text, buttonEl) {
    const dict = I18N[currentLang] || I18N.en;
    const labelEl = buttonEl.querySelector(".copy-text");

    const showSuccess = () => {
      buttonEl.classList.add("is-copied");
      if (labelEl) labelEl.textContent = dict.copied_label;
      window.clearTimeout(buttonEl._copyTimer);
      buttonEl._copyTimer = window.setTimeout(() => {
        buttonEl.classList.remove("is-copied");
        const latest = I18N[currentLang] || I18N.en;
        if (labelEl) labelEl.textContent = latest.copy_label;
      }, 1800);
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
    } catch (_) {}
    document.body.removeChild(ta);
  }

  function setupInstallTabs() {
    const tabs = document.querySelectorAll("[data-install-tab]");
    const cmdEl = document.getElementById("closing-install-cmd");
    const copyBtn = document.getElementById("closing-copy-btn");

    tabs.forEach((tab) => {
      tab.addEventListener("click", () => {
        const mode = tab.getAttribute("data-install-tab");
        if (!INSTALL_COMMANDS[mode]) return;
        activeInstallTab = mode;
        tabs.forEach((t) =>
          t.setAttribute("aria-selected", t.getAttribute("data-install-tab") === mode ? "true" : "false")
        );
        if (cmdEl) cmdEl.textContent = INSTALL_COMMANDS[mode];
        if (copyBtn) copyBtn.setAttribute("data-copy-value", INSTALL_COMMANDS[mode]);
      });
    });
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
      .catch(() => {});
  }

  document.addEventListener("DOMContentLoaded", () => {
    setupChannelNavigation();

    document.querySelectorAll("[data-set-lang]").forEach((btn) => {
      btn.addEventListener("click", () => {
        applyLanguage(btn.getAttribute("data-set-lang"), true);
        activateScene(activeScene);
      });
    });

    /* Clicking any primitive tab restarts that scene at 0% and keeps auto-cycling */
    document.querySelectorAll(".feature-tab").forEach((tab) => {
      tab.addEventListener("click", () => {
        activateScene(tab.getAttribute("data-scene"));
      });
    });

    document.querySelectorAll("[data-copy-btn]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const val = btn.getAttribute("data-copy-value") || INSTALL_COMMANDS[activeInstallTab];
        copyText(val, btn);
      });
    });

    setupInstallTabs();
    applyLanguage(currentLang, false);
    activateScene("kanban");
    startLoops();
    fetchGitHubStars();
  });
})();
