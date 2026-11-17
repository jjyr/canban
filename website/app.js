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
    const mobile = window.matchMedia("(max-width: 720px)").matches;
    const clamped = Math.max(0, Math.min(TOTAL_CHANNELS - 1, nextIndex));
    if (clamped === currentChannel && !mobile) return;

    currentChannel = clamped;

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
      if (window.matchMedia("(max-width: 720px)").matches) return;
      if (e.target && ["INPUT", "TEXTAREA", "BUTTON", "A"].includes(e.target.tagName)) return;
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

  const currentLang = document.documentElement.getAttribute("data-lang") === "zh" ? "zh" : "en";
  let activeInstallTab = "desktop";
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
    const isHome = document.body.classList.contains("home-page");
    if (isHome) setupChannelNavigation();

    document.querySelectorAll(".lang-switch a").forEach((link) => {
      link.addEventListener("click", () => {
        link.hash = window.location.hash;
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
    if (isHome) {
      activateScene("kanban");
      startLoops();
      fetchGitHubStars();
    }
  });
})();
