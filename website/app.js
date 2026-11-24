/* Copyright 2026 JJy <jjyruby@gmail.com>
 * SPDX-License-Identifier: Apache-2.0
 */

(function () {
  "use strict";

  const INSTALL_COMMANDS = {
    desktop: "curl -fsSL https://canban.dev/install.sh | bash",
    headless: "curl -fsSL https://canban.dev/install.sh | bash -s -- --headless",
  };

  /* ------------------------------------------------------------------------ */
  /* Static-page Copy Labels                                                  */
  /* ------------------------------------------------------------------------ */
  const COPY = JSON.parse(document.getElementById("page-copy").textContent);

  /* ------------------------------------------------------------------------ */
  /* Channel Paging Deck (One Screen at a Time + CRT Glitch Transition)       */
  /* ------------------------------------------------------------------------ */
  const TOTAL_CHANNELS = 4;
  const CHANNEL_OSD_NAMES = [
    "CH 01 // KANBAN",
    "CH 02 // WORKFLOW",
    "CH 03 // NATIVE CLI",
    "CH 04 // INSTALL",
  ];

  const rawLang = document.documentElement.getAttribute("data-lang") || "en";
  const currentLang = ["en", "zh", "zh-tw", "ja", "ko"].includes(rawLang) ? rawLang : "en";
  let currentChannel = 0;
  let wheelLockUntil = 0;
  let wheelAccum = 0;
  let osdTimer = null;
  let touchStartY = null;
  let activeInstallTab = "desktop";
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

  function closeLangMenus() {
    document.querySelectorAll("details.lang-menu[open]").forEach((menu) => {
      menu.removeAttribute("open");
    });
  }

  function closeNotifPopover() {
    const pop = document.getElementById("notif-popover");
    const btn = document.getElementById("notif-pill-btn");
    if (pop) pop.hidden = true;
    if (btn) btn.setAttribute("aria-expanded", "false");
  }

  function toggleNotifPopover() {
    const pop = document.getElementById("notif-popover");
    const btn = document.getElementById("notif-pill-btn");
    if (!pop || !btn) return;
    const willOpen = pop.hidden;
    pop.hidden = !willOpen;
    btn.setAttribute("aria-expanded", willOpen ? "true" : "false");
  }

  /* CH 01: Kanban Card Click -> Slide Open Pure Card Reader */
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

  /* CH 02: Full-Width Visual Workflow Studio Traversal */
  const WF_TRAVERSAL = [
    {
      node: 0,
      en: "1/5 · Backlog · @planner drafting steps",
      zh: "1/5 · Backlog · @planner 生成实施步骤",
      "zh-tw": "1/5 · Backlog · @planner 產生實作步驟",
      ja: "1/5 · Backlog · @planner が実装手順を作成中",
      ko: "1/5 · Backlog · @planner 구현 단계 작성 중",
    },
    {
      node: 1,
      vwire: 0,
      en: "2/5 · Backlog · Free Agent slot granted (4/6)",
      zh: "2/5 · Backlog · 获得空闲并发槽位 (4/6)",
      "zh-tw": "2/5 · Backlog · 取得空閒並行槽位 (4/6)",
      ja: "2/5 · Backlog · 空きエージェント枠を確保 (4/6)",
      ko: "2/5 · Backlog · 유휴 에이전트 슬롯 확보 (4/6)",
    },
    {
      node: 2,
      wire: 0,
      en: "3/5 · Develop · @developer running in worktree",
      zh: "3/5 · Develop · @developer 在独立分支编码",
      "zh-tw": "3/5 · Develop · @developer 於獨立 Worktree 開發",
      ja: "3/5 · Develop · @developer が独立 Worktree で実装中",
      ko: "3/5 · Develop · @developer 독립 Worktree에서 구현 중",
    },
    {
      node: 3,
      wire: 1,
      en: "4/5 · Review · Trusted branch verified",
      zh: "4/5 · Review · 本地受信任分支校验通过",
      "zh-tw": "4/5 · Review · 本地受信任分支校驗通過",
      ja: "4/5 · Review · 信頼済みブランチの検証完了",
      ko: "4/5 · Review · 신뢰된 브랜치 검증 완료",
    },
    {
      node: 4,
      vwire: 1,
      en: "5/5 · Review · Queue Merge Rebase ×5 & Sync ✓",
      zh: "5/5 · Review · Queue Merge 批量变基同步 ✓",
      "zh-tw": "5/5 · Review · Queue Merge 批次變基同步 ✓",
      ja: "5/5 · Review · Queue Merge で一括リベース＆同期 ✓",
      ko: "5/5 · Review · Queue Merge 일괄 리베이스 및 동기화 ✓",
    },
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
      if (banner) banner.textContent = s[currentLang] || s.en;
    };

    if (reduced) {
      WF_TRAVERSAL.forEach((_, i) => stepTo(i));
      return;
    }

    [200, 1650, 3250, 4850, 6350].forEach((ms, i) => {
      scheduleInScene(() => stepTo(i), ms);
    });
    scheduleInScene(() => {
      if (currentChannel === 1 && !document.hidden) playWorkflowScene(false);
    }, 9200);
  }

  const TERMINAL_REASONING = {
    en: "Promoted Kanban, Workflow, and Native Agent CLI to dedicated channels with interactive Notification Center.",
    zh: "已将 Kanban、Workflow 与原生 Agent CLI 拆分为三个独立频道，并接入可展开的通知中心。",
    "zh-tw": "已將 Kanban、Workflow 與原生 Agent CLI 拆分為三個獨立頻道，並接入可展開的通知中心。",
    ja: "Kanban・Workflow・ネイティブ Agent CLI を専用チャンネルに分離し、対話型の通知センターを接続しました。",
    ko: "Kanban, Workflow, 네이티브 Agent CLI를 전용 채널로 분리하고 대화형 알림 센터를 연결했습니다.",
  };

  /* CH 03: Native Agent CLI TUI Stream (Thoughts + Tool Calls + Diff) */
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
        delay: 1450,
        html: `<div class="tui-tool-head"><span class="tui-bullet is-ok">●</span><span class="tui-tool-name">Edit</span><span class="tui-tool-arg">(website/styles.css)</span></div>
               <div class="tui-diff">
                 <div class="diff-del">- --bg: #FAF9F6; /* warm paper */</div>
                 <div class="diff-add">+ --tube-bg: #151612; /* Monokai CRT phosphor */</div>
                 <div class="diff-add">+ --green: #a6e22e; --pink: #f92672; --cyan: #66d9ef;</div>
               </div>`,
      },
      {
        delay: 3200,
        html: `<div class="tui-tool-head"><span class="tui-bullet">✦</span><span class="vt-purple">Agent reasoning</span></div>
               <div class="tui-thought">${TERMINAL_REASONING[currentLang] || TERMINAL_REASONING.en}</div>`,
      },
      {
        delay: 5000,
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

  function playChannelScene(chIndex) {
    clearSceneTimers();
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const mobile = window.matchMedia("(max-width: 720px)").matches;
    if (chIndex === 0) playKanbanScene(reduced || mobile);
    else if (chIndex === 1) playWorkflowScene(reduced);
    else if (chIndex === 2) playTerminalScene(reduced);
  }

  function switchChannel(nextIndex, syncHash) {
    const mobile = window.matchMedia("(max-width: 720px)").matches;
    const clamped = Math.max(0, Math.min(TOTAL_CHANNELS - 1, nextIndex));
    if (clamped === currentChannel && !mobile) return;

    currentChannel = clamped;
    closeNotifPopover();
    closeLangMenus();

    if (syncHash !== false && window.history && window.history.replaceState) {
      const hash = window.location.pathname + window.location.search + (currentChannel === 0 ? "" : `#ch-${currentChannel}`);
      try {
        window.history.replaceState(null, "", hash);
      } catch (_) {}
    }

    const glass = document.getElementById("crt-glass");
    const osd = document.getElementById("crt-osd");
    const readout = document.getElementById("ch-readout");

    /* Trigger CRT H-Sync Glitch Burst */
    if (glass && !mobile && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
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

    if (mobile) {
      document.getElementById(`ch-${clamped}`).scrollIntoView({
        behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth",
      });
    }

    /* Flash retro green OSD label */
    if (osd) {
      osd.textContent = CHANNEL_OSD_NAMES[currentChannel];
      osd.classList.add("is-shown");
      window.clearTimeout(osdTimer);
      osdTimer = window.setTimeout(() => osd.classList.remove("is-shown"), 1150);
    }

    playChannelScene(currentChannel);
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
        if (window.matchMedia("(max-width: 720px)").matches) return;
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
        if (window.matchMedia("(max-width: 720px)").matches) return;
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
      if (e.key === "Escape") {
        closeNotifPopover();
        return;
      }
      if (window.matchMedia("(max-width: 720px)").matches) return;
      if (e.target && ["INPUT", "TEXTAREA", "BUTTON", "A", "SUMMARY"].includes(e.target.tagName)) return;
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

  function setupNotifPopover() {
    document.querySelectorAll("[data-notif-toggle]").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        toggleNotifPopover();
      });
    });

    document.querySelectorAll("[data-notif-close]").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        closeNotifPopover();
      });
    });

    document.querySelectorAll("[data-notif-action]").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const act = btn.getAttribute("data-notif-action");
        closeNotifPopover();
        if (act === "terminal") {
          switchChannel(2);
        } else if (act === "workflow") {
          switchChannel(1);
        } else if (act === "card") {
          const ws = document.getElementById("kanban-workspace");
          const card = document.getElementById("target-card-65");
          if (ws) ws.classList.add("is-drawer-open");
          if (card) card.classList.add("is-card-selected");
        }
      });
    });

    document.addEventListener("click", (e) => {
      const pop = document.getElementById("notif-popover");
      const btn = document.getElementById("notif-pill-btn");
      if (!pop || pop.hidden) return;
      if (!pop.contains(e.target) && (!btn || !btn.contains(e.target))) {
        closeNotifPopover();
      }
    });
  }

  function setupLangMenus() {
    document.addEventListener("click", (e) => {
      document.querySelectorAll("details.lang-menu[open]").forEach((menu) => {
        if (!menu.contains(e.target)) {
          menu.removeAttribute("open");
        }
      });
    });

    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        closeLangMenus();
      }
    });
  }

  function startLoops() {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    /* Subtle periodic micro-glitch on Hero Formula */
    window.setInterval(() => {
      if (document.hidden || currentChannel !== 0) return;
      if (Math.random() < 0.35) {
        const formula = document.querySelector(".hero-formula");
        if (formula) {
          formula.classList.add("is-micro-glitch");
          window.setTimeout(() => formula.classList.remove("is-micro-glitch"), 110);
        }
      }
    }, 1400);
  }

  /* ------------------------------------------------------------------------ */
  /* Copy Helpers                                                             */
  /* ------------------------------------------------------------------------ */
  function copyText(text, buttonEl) {
    const dict = COPY;
    const labelEl = buttonEl.querySelector(".copy-text");

    const showSuccess = () => {
      buttonEl.classList.add("is-copied");
      if (labelEl) labelEl.textContent = dict.copied_label;
      window.clearTimeout(buttonEl._copyTimer);
      buttonEl._copyTimer = window.setTimeout(() => {
        buttonEl.classList.remove("is-copied");
        const latest = COPY;
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
    const tabs = Array.from(document.querySelectorAll("[data-install-tab]"));
    const shellEl = document.getElementById("closing-install-shell");
    const androidEl = document.getElementById("closing-install-android");
    const cmdEl = document.getElementById("closing-install-cmd");
    const copyBtn = document.getElementById("closing-copy-btn");

    const activateTab = (mode) => {
      if (mode !== "android" && !INSTALL_COMMANDS[mode]) return;
      activeInstallTab = mode;
      tabs.forEach((t) =>
        t.setAttribute("aria-selected", t.getAttribute("data-install-tab") === mode ? "true" : "false")
      );
      if (mode === "android") {
        if (shellEl) shellEl.hidden = true;
        if (androidEl) androidEl.hidden = false;
        return;
      }
      if (shellEl) {
        shellEl.hidden = false;
        shellEl.setAttribute("aria-labelledby", `install-tab-${mode}`);
      }
      if (androidEl) androidEl.hidden = true;
      if (cmdEl) cmdEl.textContent = INSTALL_COMMANDS[mode];
      if (copyBtn) copyBtn.setAttribute("data-copy-value", INSTALL_COMMANDS[mode]);
    };

    tabs.forEach((tab, idx) => {
      tab.addEventListener("click", () => {
        activateTab(tab.getAttribute("data-install-tab"));
      });
      tab.addEventListener("keydown", (e) => {
        if (!tabs.length) return;
        let nextIdx = null;
        if (e.key === "ArrowRight") nextIdx = (idx + 1) % tabs.length;
        else if (e.key === "ArrowLeft") nextIdx = (idx - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") nextIdx = 0;
        else if (e.key === "End") nextIdx = tabs.length - 1;
        if (nextIdx === null) return;
        e.preventDefault();
        const nextTab = tabs[nextIdx];
        activateTab(nextTab.getAttribute("data-install-tab"));
        nextTab.focus();
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
    const isHome = document.body.classList.contains("home-page");
    setupLangMenus();
    if (isHome) {
      setupChannelNavigation();
      setupNotifPopover();
    }

    document.querySelectorAll(".lang-switch a").forEach((link) => {
      link.addEventListener("click", () => {
        link.hash = window.location.hash;
      });
    });

    document.querySelectorAll("[data-copy-btn]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const val =
          btn.getAttribute("data-copy-value") ||
          INSTALL_COMMANDS[activeInstallTab] ||
          INSTALL_COMMANDS.desktop;
        copyText(val, btn);
      });
    });

    setupInstallTabs();
    if (isHome) {
      playChannelScene(currentChannel);
      /* On mobile all screens are visible stacked, so populate Workflow + Terminal too */
      if (window.matchMedia("(max-width: 720px)").matches) {
        playWorkflowScene(true);
        playTerminalScene(true);
      }
      startLoops();
      fetchGitHubStars();
    }
  });
})();
