/* Copyright 2026 JJy <jjyruby@gmail.com>
 * SPDX-License-Identifier: Apache-2.0
 */

(function () {
  "use strict";

  const INSTALL_COMMANDS = {
    desktop: "curl -fsSL https://canban.dev/install.sh | bash",
    headless: "curl -fsSL https://canban.dev/install.sh | bash -s -- --headless",
  };

  const copyNode = document.getElementById("page-copy");
  const COPY = copyNode
    ? JSON.parse(copyNode.textContent)
    : { copy_label: "Copy", copied_label: "Copied" };

  /* ------------------------------------------------------------------------ */
  /* Language Dropdown (<details class="lang-menu">)                          */
  /* ------------------------------------------------------------------------ */
  function closeLangMenus() {
    document.querySelectorAll("details.lang-menu[open]").forEach((menu) => {
      menu.removeAttribute("open");
    });
  }

  document.addEventListener("click", (event) => {
    const insideLang = event.target.closest("details.lang-menu");
    if (!insideLang) {
      closeLangMenus();
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeLangMenus();
    }
  });

  /* ------------------------------------------------------------------------ */
  /* 6-Tab Real-Screenshot Product Stage                                      */
  /* ------------------------------------------------------------------------ */
  const showcaseTabs = Array.from(document.querySelectorAll("[data-showcase-tab]"));
  const showcasePanels = Array.from(document.querySelectorAll("[data-showcase-panel]"));
  const showcaseCaption = document.getElementById("showcase-caption");

  function selectShowcaseTab(tabId, focusTab) {
    if (!showcaseTabs.length) return;
    let activeCaption = "";
    showcaseTabs.forEach((btn) => {
      const isMatch = btn.getAttribute("data-showcase-tab") === tabId;
      btn.setAttribute("aria-selected", isMatch ? "true" : "false");
      btn.tabIndex = isMatch ? 0 : -1;
      if (isMatch) {
        activeCaption = btn.getAttribute("data-caption") || "";
        if (focusTab) btn.focus();
      }
    });
    showcasePanels.forEach((panel) => {
      const isMatch = panel.getAttribute("data-showcase-panel") === tabId;
      panel.hidden = !isMatch;
    });
    if (showcaseCaption && activeCaption) {
      showcaseCaption.textContent = activeCaption;
    }
  }

  if (showcaseTabs.length) {
    showcaseTabs.forEach((btn, idx) => {
      btn.addEventListener("click", () => {
        selectShowcaseTab(btn.getAttribute("data-showcase-tab"), false);
      });
      btn.addEventListener("keydown", (event) => {
        let nextIdx = null;
        if (event.key === "ArrowRight") nextIdx = (idx + 1) % showcaseTabs.length;
        else if (event.key === "ArrowLeft")
          nextIdx = (idx - 1 + showcaseTabs.length) % showcaseTabs.length;
        else if (event.key === "Home") nextIdx = 0;
        else if (event.key === "End") nextIdx = showcaseTabs.length - 1;
        if (nextIdx !== null) {
          event.preventDefault();
          selectShowcaseTab(
            showcaseTabs[nextIdx].getAttribute("data-showcase-tab"),
            true
          );
        }
      });
    });
  }

  /* ------------------------------------------------------------------------ */
  /* Install Tabs (Desktop / Headless / Android) — Synced Across Page         */
  /* ------------------------------------------------------------------------ */
  function setInstallTab(mode) {
    const valid = mode === "headless" || mode === "android" ? mode : "desktop";
    const isAndroid = valid === "android";

    document.querySelectorAll("[data-install-tab]").forEach((btn) => {
      const isMatch = btn.getAttribute("data-install-tab") === valid;
      btn.classList.toggle("is-active", isMatch);
      btn.setAttribute("aria-selected", isMatch ? "true" : "false");
    });

    document.querySelectorAll("[data-install-shell]").forEach((shell) => {
      shell.hidden = isAndroid;
      if (!isAndroid) {
        const cmdEl = shell.querySelector("[data-install-cmd]");
        const copyEl = shell.querySelector("[data-copy-btn]");
        const cmd = INSTALL_COMMANDS[valid];
        if (cmdEl) cmdEl.textContent = cmd;
        if (copyEl) copyEl.setAttribute("data-copy-value", cmd);
      }
    });

    document.querySelectorAll("[data-install-android]").forEach((panel) => {
      panel.hidden = !isAndroid;
    });
  }

  document.querySelectorAll("[data-install-tab]").forEach((btn) => {
    btn.addEventListener("click", () => {
      setInstallTab(btn.getAttribute("data-install-tab"));
    });
  });

  /* ------------------------------------------------------------------------ */
  /* Copy to Clipboard                                                        */
  /* ------------------------------------------------------------------------ */
  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise((resolve, reject) => {
      const area = document.createElement("textarea");
      area.value = text;
      area.setAttribute("readonly", "");
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      try {
        document.execCommand("copy") ? resolve() : reject(new Error("copy failed"));
      } catch (err) {
        reject(err);
      } finally {
        document.body.removeChild(area);
      }
    });
  }

  document.querySelectorAll("[data-copy-btn]").forEach((btn) => {
    let resetTimer = null;
    btn.addEventListener("click", () => {
      const value = btn.getAttribute("data-copy-value") || "";
      if (!value) return;
      copyText(value)
        .then(() => {
          const label = btn.querySelector(".copy-text");
          if (label) label.textContent = COPY.copied_label;
          btn.classList.add("is-copied");
          if (resetTimer) window.clearTimeout(resetTimer);
          resetTimer = window.setTimeout(() => {
            if (label) label.textContent = COPY.copy_label;
            btn.classList.remove("is-copied");
          }, 1600);
        })
        .catch(() => {});
    });
  });

  /* ------------------------------------------------------------------------ */
  /* GitHub Star Count (Graceful Optional Enhancement)                        */
  /* ------------------------------------------------------------------------ */
  const starNodes = document.querySelectorAll(".star-count");
  if (starNodes.length && typeof window.fetch === "function") {
    window
      .fetch("https://api.github.com/repos/jjyr/canban", {
        headers: { Accept: "application/vnd.github+json" },
      })
      .then((res) => (res.ok ? res.json() : null))
      .then((data) => {
        if (!data || typeof data.stargazers_count !== "number") return;
        const count = data.stargazers_count;
        const formatted =
          count >= 1000 ? (count / 1000).toFixed(1).replace(/\.0$/, "") + "k" : String(count);
        starNodes.forEach((node) => {
          node.textContent = formatted;
        });
      })
      .catch(() => {});
  }
})();
